//! Scoped, nested timers.
//!
//! ```
//! use drafftink_profile as profile;
//! profile::set_enabled(true);
//! {
//!     let _frame = profile::span("encode");
//!     let _inner = profile::span("encode_text"); // nested: a child of "encode"
//! }
//! profile::set_enabled(false);
//! ```
//!
//! Cost model:
//! - `default-features = false` (no `enabled` feature): `Span` is zero-sized, every function is
//!   an empty inline function.
//! - Compiled in, run-time flag off (the default): one relaxed atomic load per span.
//! - On: two clock reads and a push/pop on a thread-local stack. A finished span goes into a
//!   thread-local buffer. The buffer moves to the shared bounded ring when the thread's
//!   outermost span ends, so the shared lock is taken once per top-level span, not once per span.
//!
//! A span's self time is its time less its children's on the same thread. Its total counts
//! only its outermost occurrence on a thread's stack, so recursion is not counted twice.

use std::collections::{BTreeMap, VecDeque};
#[cfg(feature = "enabled")]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Finished spans kept for the Chrome trace; older ones are dropped (and counted).
pub const TRACE_CAPACITY: usize = 32_768;
/// Deepest span nesting recorded on one thread; deeper spans are ignored.
pub const MAX_DEPTH: usize = 256;
/// Distinct span names aggregated; further new names are folded into `other`.
pub const MAX_NAMES: usize = 1024;
/// The name that spans past [`MAX_NAMES`] are aggregated under.
pub const OTHER: &str = "other";
/// Finished spans a thread buffers before it flushes without waiting for its root span.
#[cfg(feature = "enabled")]
const FLUSH_AT: usize = 256;
static ON: AtomicBool = AtomicBool::new(false);

/// Is recording on? Always false when the `enabled` feature is off.
#[inline]
pub fn enabled() -> bool {
    cfg!(feature = "enabled") && ON.load(Ordering::Relaxed)
}

/// Turn recording of spans, phases and frames on or off. What was recorded stays until
/// [`crate::reset`]. Does nothing when the `enabled` feature is off.
pub fn set_enabled(on: bool) {
    if cfg!(feature = "enabled") {
        ON.store(on, Ordering::Relaxed);
    }
}

/// Whether the timing code is compiled into this build (the `enabled` feature).
pub const COMPILED: bool = cfg!(feature = "enabled");

/// One finished span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub name: &'static str,
    pub tid: u32,
    pub start_ns: u64,
    pub dur_ns: u64,
}

/// Aggregate per span name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Agg {
    pub count: u64,
    pub total_ns: u64,
    pub self_ns: u64,
    pub max_ns: u64,
}

impl Agg {
    #[cfg(feature = "enabled")]
    fn merge(&mut self, o: &Agg) {
        self.count = self.count.saturating_add(o.count);
        self.total_ns = self.total_ns.saturating_add(o.total_ns);
        self.self_ns = self.self_ns.saturating_add(o.self_ns);
        self.max_ns = self.max_ns.max(o.max_ns);
    }
}

pub(crate) struct Global {
    ring: VecDeque<Event>,
    agg: BTreeMap<&'static str, Agg>,
    dropped: u64,
    threads: Vec<(u32, String)>,
}

static GLOBAL: Mutex<Global> =
    Mutex::new(Global { ring: VecDeque::new(), agg: BTreeMap::new(), dropped: 0, threads: Vec::new() });

fn global() -> MutexGuard<'static, Global> {
    GLOBAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running span; it ends when dropped, also while a panic unwinds.
#[must_use = "a span measures until it is dropped"]
pub struct Span {
    /// Stack depth of this span, or `None` when it is not recording.
    #[cfg(feature = "enabled")]
    depth: Option<u32>,
}

impl Drop for Span {
    #[inline]
    fn drop(&mut self) {
        #[cfg(feature = "enabled")]
        if let Some(depth) = self.depth {
            imp::close(depth);
        }
    }
}

/// Start a span called `name` on this thread. Use short snake_case literals such as
/// `"encode"` or `"hit_test"`; they are the keys of `PerfStats.spans`.
#[inline]
pub fn span(name: &'static str) -> Span {
    #[cfg(feature = "enabled")]
    {
        Span { depth: if ON.load(Ordering::Relaxed) { imp::open(name) } else { None } }
    }
    #[cfg(not(feature = "enabled"))]
    {
        let _ = name;
        Span {}
    }
}

/// Run `f` as a span called `name`.
#[inline]
pub fn scope<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let _span = span(name);
    f()
}

/// Move this thread's finished spans to the shared ring. Reports call it for the calling
/// thread; other threads flush when their outermost span ends.
pub fn flush_thread() {
    #[cfg(feature = "enabled")]
    imp::flush_current();
}

/// Forget recorded spans and aggregates (the calling thread's buffer too).
pub(crate) fn reset() {
    {
        let mut g = global();
        g.ring.clear();
        g.agg.clear();
        g.dropped = 0;
    }
    #[cfg(feature = "enabled")]
    imp::clear_current();
}

/// A copy of the shared state for reports.
pub(crate) struct SpanData {
    pub events: Vec<Event>,
    pub agg: Vec<(&'static str, Agg)>,
    pub dropped: u64,
    pub threads: Vec<(u32, String)>,
}

pub(crate) fn data() -> SpanData {
    flush_thread();
    let g = global();
    SpanData {
        events: g.ring.iter().copied().collect(),
        agg: g.agg.iter().map(|(k, v)| (*k, *v)).collect(),
        dropped: g.dropped,
        threads: g.threads.clone(),
    }
}

#[cfg(feature = "enabled")]
mod imp {
    use std::cell::RefCell;

    use super::{
        Agg, AtomicU32, BTreeMap, Event, FLUSH_AT, MAX_DEPTH, MAX_NAMES, OTHER, Ordering, TRACE_CAPACITY, global,
    };
    use crate::clock::now_ns;

    struct Frame {
        name: &'static str,
        start_ns: u64,
        child_ns: u64,
    }

    struct Local {
        tid: u32,
        stack: Vec<Frame>,
        events: Vec<Event>,
        agg: BTreeMap<&'static str, Agg>,
    }

    static NEXT_TID: AtomicU32 = AtomicU32::new(1);

    impl Local {
        fn new() -> Self {
            let tid = NEXT_TID.fetch_add(1, Ordering::Relaxed);
            #[cfg(not(target_arch = "wasm32"))]
            let name = std::thread::current().name().map(str::to_owned);
            #[cfg(target_arch = "wasm32")]
            let name: Option<String> = None;
            let name = name.unwrap_or_else(|| format!("thread {tid}"));
            let mut g = global();
            if g.threads.len() < 256 {
                g.threads.push((tid, name));
            }
            Self { tid, stack: Vec::new(), events: Vec::new(), agg: BTreeMap::new() }
        }

        fn flush(&mut self) {
            if self.events.is_empty() && self.agg.is_empty() {
                return;
            }
            let mut g = global();
            for e in self.events.drain(..) {
                if g.ring.len() >= TRACE_CAPACITY {
                    g.ring.pop_front();
                    g.dropped = g.dropped.saturating_add(1);
                }
                g.ring.push_back(e);
            }
            for (name, a) in std::mem::take(&mut self.agg) {
                let key = if g.agg.contains_key(name) || g.agg.len() < MAX_NAMES { name } else { OTHER };
                g.agg.entry(key).or_default().merge(&a);
            }
        }
    }

    impl Drop for Local {
        fn drop(&mut self) {
            self.flush();
        }
    }

    thread_local! {
        static LOCAL: RefCell<Local> = RefCell::new(Local::new());
    }

    /// Push a frame; the depth to pass to [`close`], or `None` when the span is not recorded.
    pub(super) fn open(name: &'static str) -> Option<u32> {
        LOCAL
            .try_with(|l| {
                let mut l = l.try_borrow_mut().ok()?;
                if l.stack.len() >= MAX_DEPTH {
                    return None;
                }
                let depth = u32::try_from(l.stack.len()).ok()?;
                l.stack.push(Frame { name, start_ns: now_ns(), child_ns: 0 });
                Some(depth)
            })
            .ok()
            .flatten()
    }

    pub(super) fn close(depth: u32) {
        let end = now_ns();
        let _ = LOCAL.try_with(|l| {
            let Ok(mut l) = l.try_borrow_mut() else {
                return;
            };
            let depth = depth as usize;
            if depth >= l.stack.len() {
                return;
            }
            // Spans above this one whose guards were leaked are discarded.
            l.stack.truncate(depth + 1);
            let Some(f) = l.stack.pop() else { return };
            let dur = end.saturating_sub(f.start_ns);
            let self_ns = dur.saturating_sub(f.child_ns);
            let outermost = !l.stack.iter().any(|x| x.name == f.name);
            if let Some(parent) = l.stack.last_mut() {
                parent.child_ns = parent.child_ns.saturating_add(dur);
            }
            let key = if l.agg.contains_key(f.name) || l.agg.len() < MAX_NAMES { f.name } else { OTHER };
            let a = l.agg.entry(key).or_default();
            a.count = a.count.saturating_add(1);
            if outermost {
                a.total_ns = a.total_ns.saturating_add(dur);
            }
            a.self_ns = a.self_ns.saturating_add(self_ns);
            a.max_ns = a.max_ns.max(dur);
            let tid = l.tid;
            l.events.push(Event { name: f.name, tid, start_ns: f.start_ns, dur_ns: dur });
            if l.stack.is_empty() || l.events.len() >= FLUSH_AT {
                l.flush();
            }
        });
    }

    pub(super) fn flush_current() {
        let _ = LOCAL.try_with(|l| {
            if let Ok(mut l) = l.try_borrow_mut() {
                l.flush();
            }
        });
    }

    pub(super) fn clear_current() {
        let _ = LOCAL.try_with(|l| {
            if let Ok(mut l) = l.try_borrow_mut() {
                l.events.clear();
                l.agg.clear();
            }
        });
    }
}

#[cfg(all(test, feature = "enabled"))]
mod tests {
    use super::*;
    use crate::test_lock;

    fn row(name: &str) -> Option<Agg> {
        data().agg.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
    }

    fn busy(ns: u64) {
        let t = crate::clock::now_ns();
        while crate::clock::now_ns() - t < ns {}
    }

    #[test]
    fn disabled_records_nothing() {
        let _l = test_lock();
        crate::reset();
        set_enabled(false);
        {
            let _s = span("sp_off");
        }
        assert!(row("sp_off").is_none());
        assert!(data().events.is_empty());
    }

    #[test]
    fn nested_spans_have_self_and_total_time() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        {
            let _outer = span("sp_outer");
            busy(200_000);
            {
                let _inner = span("sp_inner");
                busy(400_000);
            }
        }
        set_enabled(false);
        let outer = row("sp_outer").unwrap();
        let inner = row("sp_inner").unwrap();
        assert_eq!((outer.count, inner.count), (1, 1));
        assert!(inner.total_ns >= 400_000);
        assert!(outer.total_ns >= inner.total_ns + 200_000);
        // self time excludes the child
        assert_eq!(outer.self_ns, outer.total_ns - inner.total_ns);
        assert_eq!(inner.self_ns, inner.total_ns);
    }

    #[test]
    fn recursion_counts_total_once() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        fn rec(n: u32) {
            let _s = span("sp_rec");
            busy(100_000);
            if n > 0 {
                rec(n - 1);
            }
        }
        rec(3);
        set_enabled(false);
        let r = row("sp_rec").unwrap();
        assert_eq!(r.count, 4);
        // total is the outermost span only; self times add up to it
        assert_eq!(r.total_ns, r.self_ns);
        assert!(r.total_ns >= 400_000);
    }

    #[test]
    fn events_are_ordered_and_nested_in_time() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        scope("sp_a", || {
            scope("sp_b", || busy(50_000));
        });
        set_enabled(false);
        let d = data();
        let a = d.events.iter().find(|e| e.name == "sp_a").unwrap();
        let b = d.events.iter().find(|e| e.name == "sp_b").unwrap();
        assert!(b.start_ns >= a.start_ns);
        assert!(b.start_ns + b.dur_ns <= a.start_ns + a.dur_ns);
        assert_eq!(a.tid, b.tid);
    }

    #[test]
    fn out_of_order_and_leaked_guards_do_not_corrupt_the_stack() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        let a = span("sp_leak_a");
        let b = span("sp_leak_b");
        drop(a); // closes a and discards b's frame
        drop(b); // nothing left to close
        {
            let _c = span("sp_after");
        }
        set_enabled(false);
        assert_eq!(row("sp_leak_a").unwrap().count, 1);
        assert!(row("sp_leak_b").is_none());
        assert_eq!(row("sp_after").unwrap().count, 1);
    }

    #[test]
    fn spans_from_other_threads_are_flushed() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        std::thread::Builder::new()
            .name("worker-x".into())
            .spawn(|| {
                let _s = span("sp_thread");
            })
            .unwrap()
            .join()
            .unwrap();
        set_enabled(false);
        let d = data();
        let e = d.events.iter().find(|e| e.name == "sp_thread").unwrap();
        assert!(d.threads.iter().any(|(t, n)| *t == e.tid && n == "worker-x"));
    }

    #[test]
    fn ring_is_bounded_and_counts_drops() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        for _ in 0..(TRACE_CAPACITY + 100) {
            let _s = span("sp_many");
        }
        set_enabled(false);
        let d = data();
        assert_eq!(d.events.len(), TRACE_CAPACITY);
        assert_eq!(d.dropped, 100);
        // the aggregate still counts every span
        assert_eq!(row("sp_many").unwrap().count, (TRACE_CAPACITY + 100) as u64);
    }

    #[test]
    fn depth_is_bounded() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        fn deep(n: usize) {
            let _s = span("sp_deep");
            if n > 0 {
                deep(n - 1);
            }
        }
        deep(MAX_DEPTH + 50);
        set_enabled(false);
        assert_eq!(row("sp_deep").unwrap().count, MAX_DEPTH as u64);
    }

    #[test]
    fn a_span_open_when_recording_stops_still_closes() {
        let _l = test_lock();
        crate::reset();
        set_enabled(true);
        let s = span("sp_midway");
        set_enabled(false);
        drop(s);
        assert_eq!(row("sp_midway").unwrap().count, 1);
    }
}
