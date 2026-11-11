//! Work counters, gauges and redraw reasons.
//!
//! These are always compiled in (no cargo feature, no run-time flag). A count is one relaxed
//! atomic add plus one thread-local add, a few nanoseconds.
//!
//! Every update goes to two places:
//! - process-wide atomics, which `PerfStats` exports, and
//! - thread-local cells, which [`WorkScope`] reads. A test that runs on its own thread sees
//!   only its own work, however many tests run in parallel. That is what makes
//!   "this operation encoded 3 elements" and "no redraw happened in this window"
//!   deterministic: no clock, no shared state.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::clock::now_ns;
use crate::frame::FrameStats;
use crate::names::{Counter, Gauge, RedrawReason, RedrawReasons};
use crate::stats::PerfStats;

/// Named (ad hoc) counters kept per process and per thread; further new names are ignored.
pub(crate) const MAX_NAMED: usize = 64;
/// Redraw events kept for `PerfStats.redraws.recent` and the trace.
pub(crate) const RECENT_REDRAWS: usize = 256;
/// Redraw reasons a thread remembers for [`WorkScope::redraw_reasons`].
const LOG_CAP: usize = 4096;

static COUNTERS: [AtomicU64; Counter::COUNT] = [const { AtomicU64::new(0) }; Counter::COUNT];
static GAUGES: [AtomicU64; Gauge::COUNT] = [const { AtomicU64::new(0) }; Gauge::COUNT];
static REDRAWS: [AtomicU64; RedrawReason::COUNT] = [const { AtomicU64::new(0) }; RedrawReason::COUNT];
/// Reasons requested since the last `begin_frame`, one bit per `RedrawReason`.
static PENDING: AtomicU32 = AtomicU32::new(0);
static NAMED: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());
static RECENT: Mutex<VecDeque<(u64, RedrawReason)>> = Mutex::new(VecDeque::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Local {
    counters: [Cell<u64>; Counter::COUNT],
    gauges: [Cell<u64>; Gauge::COUNT],
    redraws: [Cell<u64>; RedrawReason::COUNT],
    named: RefCell<Vec<(&'static str, u64)>>,
    log: RefCell<VecDeque<RedrawReason>>,
    /// Number of entries dropped from the front of `log`.
    log_base: Cell<u64>,
}

thread_local! {
    static LOCAL: Local = const {
        Local {
            counters: [const { Cell::new(0) }; Counter::COUNT],
            gauges: [const { Cell::new(0) }; Gauge::COUNT],
            redraws: [const { Cell::new(0) }; RedrawReason::COUNT],
            named: RefCell::new(Vec::new()),
            log: RefCell::new(VecDeque::new()),
            log_base: Cell::new(0),
        }
    };
}

/// Add `n` to a counter.
#[inline]
pub fn count(counter: Counter, n: u64) {
    if let Some(a) = COUNTERS.get(counter.index()) {
        a.fetch_add(n, Ordering::Relaxed);
    }
    let _ = LOCAL.try_with(|l| {
        if let Some(c) = l.counters.get(counter.index()) {
            c.set(c.get().saturating_add(n));
        }
    });
}

/// Add 1 to a counter.
#[inline]
pub fn inc(counter: Counter) {
    count(counter, 1);
}

/// Add `n` to an ad hoc counter that has no [`Counter`] variant, for a quick local check.
/// It takes a lock, so keep it out of hot loops. At most `MAX_NAMED` (64) distinct names are kept.
pub fn count_named(name: &'static str, n: u64) {
    bump_named(&mut lock(&NAMED), name, n);
    let _ = LOCAL.try_with(|l| bump_named(&mut l.named.borrow_mut(), name, n));
}

fn bump_named(v: &mut Vec<(&'static str, u64)>, name: &'static str, n: u64) {
    if let Some(e) = v.iter_mut().find(|e| e.0 == name) {
        e.1 = e.1.saturating_add(n);
    } else if v.len() < MAX_NAMED {
        v.push((name, n));
    }
}

/// Set a gauge to its latest value.
#[inline]
pub fn set_gauge(gauge: Gauge, value: u64) {
    if let Some(a) = GAUGES.get(gauge.index()) {
        a.store(value, Ordering::Relaxed);
    }
    let _ = LOCAL.try_with(|l| {
        if let Some(c) = l.gauges.get(gauge.index()) {
            c.set(value);
        }
    });
}

/// Record that a redraw is requested because of `reason`.
///
/// Call it where the decision is made (the idle gate, the repaint-after handler, the network
/// wakeup). The next `begin_frame` takes the pending reasons, so every frame says why it was
/// drawn; a frame with no reason is counted as unexplained and fails the idle gate.
pub fn redraw(reason: RedrawReason) {
    if let Some(a) = REDRAWS.get(reason.index()) {
        a.fetch_add(1, Ordering::Relaxed);
    }
    PENDING.fetch_or(RedrawReasons::of(reason).bits(), Ordering::Relaxed);
    {
        let mut recent = lock(&RECENT);
        if recent.len() >= RECENT_REDRAWS {
            recent.pop_front();
        }
        recent.push_back((now_ns(), reason));
    }
    let _ = LOCAL.try_with(|l| {
        if let Some(c) = l.redraws.get(reason.index()) {
            c.set(c.get().saturating_add(1));
        }
        let mut log = l.log.borrow_mut();
        if log.len() >= LOG_CAP {
            log.pop_front();
            l.log_base.set(l.log_base.get().saturating_add(1));
        }
        log.push_back(reason);
    });
}

/// Take (and clear) the reasons requested since the last call.
pub(crate) fn take_pending_reasons() -> RedrawReasons {
    RedrawReasons::from_bits(PENDING.swap(0, Ordering::Relaxed))
}

pub(crate) fn global_counters() -> [u64; Counter::COUNT] {
    std::array::from_fn(|i| COUNTERS.get(i).map_or(0, |a| a.load(Ordering::Relaxed)))
}

pub(crate) fn global_gauges() -> [u64; Gauge::COUNT] {
    std::array::from_fn(|i| GAUGES.get(i).map_or(0, |a| a.load(Ordering::Relaxed)))
}

pub(crate) fn global_redraws() -> [u64; RedrawReason::COUNT] {
    std::array::from_fn(|i| REDRAWS.get(i).map_or(0, |a| a.load(Ordering::Relaxed)))
}

pub(crate) fn global_named() -> Vec<(&'static str, u64)> {
    lock(&NAMED).clone()
}

pub(crate) fn recent_redraws() -> Vec<(u64, RedrawReason)> {
    lock(&RECENT).iter().copied().collect()
}

/// Zero the process-wide counters, gauges, redraw counts and the recent-redraw log.
/// Thread-local state is untouched: a [`WorkScope`] measures differences and is unaffected.
pub(crate) fn reset_global() {
    for a in COUNTERS.iter().chain(GAUGES.iter()).chain(REDRAWS.iter()) {
        a.store(0, Ordering::Relaxed);
    }
    PENDING.store(0, Ordering::Relaxed);
    lock(&NAMED).clear();
    lock(&RECENT).clear();
}

/// A measuring window over the work done by the **current thread**, for tests.
///
/// ```
/// use drafftink_profile::{Counter, RedrawReason, WorkScope, count, redraw};
///
/// let work = WorkScope::begin();
/// count(Counter::ElementsEncoded, 3);
/// redraw(RedrawReason::Command);
/// assert_eq!(work.count(Counter::ElementsEncoded), 3);
/// assert_eq!(work.redraws(), 1);
/// assert_eq!(work.redraw_reasons(), vec![RedrawReason::Command]);
/// ```
///
/// Only the thread that called [`WorkScope::begin`] is measured (the type is `!Send`), so
/// tests that run in parallel do not see each other. Work an operation hands to other threads
/// is not seen; assert on the process-wide `snapshot()` for that, in a test that runs alone.
#[derive(Debug)]
pub struct WorkScope {
    counters: [u64; Counter::COUNT],
    redraws: [u64; RedrawReason::COUNT],
    named: Vec<(&'static str, u64)>,
    log_seq: u64,
    _not_send: PhantomData<*const ()>,
}

impl WorkScope {
    /// Start measuring from now.
    pub fn begin() -> Self {
        LOCAL
            .try_with(|l| Self {
                counters: std::array::from_fn(|i| l.counters.get(i).map_or(0, Cell::get)),
                redraws: std::array::from_fn(|i| l.redraws.get(i).map_or(0, Cell::get)),
                named: l.named.borrow().clone(),
                log_seq: l.log_base.get().saturating_add(l.log.borrow().len() as u64),
                _not_send: PhantomData,
            })
            .unwrap_or(Self {
                counters: [0; Counter::COUNT],
                redraws: [0; RedrawReason::COUNT],
                named: Vec::new(),
                log_seq: 0,
                _not_send: PhantomData,
            })
    }

    /// A window that starts when the current thread did: its numbers are everything this thread has done.
    fn since_thread_start() -> Self {
        Self {
            counters: [0; Counter::COUNT],
            redraws: [0; RedrawReason::COUNT],
            named: Vec::new(),
            log_seq: 0,
            _not_send: PhantomData,
        }
    }

    /// How much `counter` grew since `begin`.
    pub fn count(&self, counter: Counter) -> u64 {
        let base = self.counters.get(counter.index()).copied().unwrap_or(0);
        LOCAL.try_with(|l| l.counters.get(counter.index()).map_or(0, Cell::get)).unwrap_or(0).saturating_sub(base)
    }

    /// How much the ad hoc counter `name` grew since `begin`.
    pub fn named(&self, name: &str) -> u64 {
        let now = LOCAL.try_with(|l| l.named.borrow().iter().find(|e| e.0 == name).map_or(0, |e| e.1)).unwrap_or(0);
        let base = self.named.iter().find(|e| e.0 == name).map_or(0, |e| e.1);
        now.saturating_sub(base)
    }

    /// The current value of a gauge on this thread.
    pub fn gauge(&self, gauge: Gauge) -> u64 {
        LOCAL.try_with(|l| l.gauges.get(gauge.index()).map_or(0, Cell::get)).unwrap_or(0)
    }

    /// Redraws requested since `begin`, all reasons.
    pub fn redraws(&self) -> u64 {
        RedrawReason::ALL.iter().map(|r| self.redraws_for(*r)).sum()
    }

    /// Redraws requested since `begin` for one reason.
    pub fn redraws_for(&self, reason: RedrawReason) -> u64 {
        let base = self.redraws.get(reason.index()).copied().unwrap_or(0);
        LOCAL.try_with(|l| l.redraws.get(reason.index()).map_or(0, Cell::get)).unwrap_or(0).saturating_sub(base)
    }

    /// The reasons of the redraws requested since `begin`, in order (at most the last 4096).
    pub fn redraw_reasons(&self) -> Vec<RedrawReason> {
        LOCAL
            .try_with(|l| {
                let skip = usize::try_from(self.log_seq.saturating_sub(l.log_base.get())).unwrap_or(usize::MAX);
                l.log.borrow().iter().skip(skip).copied().collect()
            })
            .unwrap_or_default()
    }

    /// What this scope saw as a [`PerfStats`]: the counter deltas, the gauges as they are now and the redraws
    /// requested since `begin`. There are no frames or spans. A work-counter test wraps it in a
    /// [`crate::ScenarioReport`] to check its numbers against `perf/budgets.json`.
    pub fn stats(&self) -> PerfStats {
        let mut counters = [0; Counter::COUNT];
        for c in Counter::ALL {
            if let Some(slot) = counters.get_mut(c.index()) {
                *slot = self.count(*c);
            }
        }
        let mut gauges = [0; Gauge::COUNT];
        for g in Gauge::ALL {
            if let Some(slot) = gauges.get_mut(g.index()) {
                *slot = self.gauge(*g);
            }
        }
        let mut redraws = [0; RedrawReason::COUNT];
        for r in RedrawReason::ALL {
            if let Some(slot) = redraws.get_mut(r.index()) {
                *slot = self.redraws_for(*r);
            }
        }
        PerfStats::assemble(FrameStats::default(), counters, gauges, redraws, Vec::new(), Vec::new(), Vec::new(), 0)
    }

    /// Every counter that grew since `begin`, as `name=delta` text, for assertion messages.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Counter::ALL
            .iter()
            .filter(|c| self.count(**c) > 0)
            .map(|c| format!("{}={}", c.name(), self.count(*c)))
            .collect();
        for r in RedrawReason::ALL {
            let n = self.redraws_for(*r);
            if n > 0 {
                parts.push(format!("redraw.{}={n}", r.name()));
            }
        }
        if parts.is_empty() { "no work".to_string() } else { parts.join(" ") }
    }
}

/// The work of the current thread since it started, as a [`PerfStats`] (counters, gauges, redraws; no
/// frames or spans). For a process that runs many independent sessions on different threads (a test suite,
/// the testkit): `snapshot()` adds them all up, this one shows a single thread's own numbers.
pub fn snapshot_thread() -> PerfStats {
    WorkScope::since_thread_start().stats()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thread_snapshot_holds_this_threads_work_only() {
        count(Counter::ElementsEncoded, 4);
        std::thread::spawn(|| count(Counter::ElementsEncoded, 100)).join().unwrap();
        let first = snapshot_thread().counters["elements_encoded"];
        count(Counter::ElementsEncoded, 3);
        assert_eq!(snapshot_thread().counters["elements_encoded"], first + 3);
        assert_eq!(first, 4);
    }

    #[test]
    fn scope_sees_only_its_own_window() {
        count(Counter::ElementsEncoded, 5);
        let w = WorkScope::begin();
        assert_eq!(w.count(Counter::ElementsEncoded), 0);
        count(Counter::ElementsEncoded, 2);
        inc(Counter::ElementsEncoded);
        assert_eq!(w.count(Counter::ElementsEncoded), 3);
        assert_eq!(w.count(Counter::ElementsCulled), 0);
        assert_eq!(w.summary(), "elements_encoded=3");
    }

    #[test]
    fn scopes_nest_and_overlap() {
        let outer = WorkScope::begin();
        inc(Counter::CacheHit);
        let inner = WorkScope::begin();
        inc(Counter::CacheHit);
        inc(Counter::CacheMiss);
        assert_eq!(outer.count(Counter::CacheHit), 2);
        assert_eq!(inner.count(Counter::CacheHit), 1);
        assert_eq!(inner.count(Counter::CacheMiss), 1);
    }

    #[test]
    fn other_threads_do_not_leak_into_a_scope() {
        // redraw() sets the process-wide pending mask that the frame tests read
        let _l = crate::test_lock();
        let w = WorkScope::begin();
        std::thread::spawn(|| {
            count(Counter::ElementsEncoded, 100);
            redraw(RedrawReason::Collab);
        })
        .join()
        .unwrap();
        assert_eq!(w.count(Counter::ElementsEncoded), 0);
        assert_eq!(w.redraws(), 0);
    }

    #[test]
    fn redraw_reasons_are_counted_and_ordered() {
        // redraw() sets the process-wide pending mask that the frame tests read
        let _l = crate::test_lock();
        let w = WorkScope::begin();
        assert_eq!(w.redraws(), 0);
        assert!(w.redraw_reasons().is_empty());
        redraw(RedrawReason::Gesture);
        redraw(RedrawReason::HoverChange);
        redraw(RedrawReason::Gesture);
        assert_eq!(w.redraws(), 3);
        assert_eq!(w.redraws_for(RedrawReason::Gesture), 2);
        assert_eq!(w.redraws_for(RedrawReason::Timer), 0);
        assert_eq!(w.redraw_reasons(), vec![RedrawReason::Gesture, RedrawReason::HoverChange, RedrawReason::Gesture]);
        let later = WorkScope::begin();
        redraw(RedrawReason::Timer);
        assert_eq!(later.redraw_reasons(), vec![RedrawReason::Timer]);
        assert_eq!(w.redraws(), 4);
    }

    #[test]
    fn redraw_log_survives_wrapping() {
        // redraw() sets the process-wide pending mask that the frame tests read
        let _l = crate::test_lock();
        let w = WorkScope::begin();
        for _ in 0..(LOG_CAP + 10) {
            redraw(RedrawReason::Ui);
        }
        let late = WorkScope::begin();
        redraw(RedrawReason::Load);
        assert_eq!(late.redraw_reasons(), vec![RedrawReason::Load]);
        assert_eq!(w.redraws(), (LOG_CAP + 11) as u64);
        assert_eq!(w.redraw_reasons().len(), LOG_CAP);
    }

    #[test]
    fn named_counters_and_gauges() {
        let w = WorkScope::begin();
        count_named("test_only_thing", 2);
        count_named("test_only_thing", 3);
        assert_eq!(w.named("test_only_thing"), 5);
        assert_eq!(w.named("never_counted"), 0);
        set_gauge(Gauge::ElementsTotal, 1000);
        assert_eq!(w.gauge(Gauge::ElementsTotal), 1000);
    }

    #[test]
    fn summary_reports_redraws() {
        // redraw() sets the process-wide pending mask that the frame tests read
        let _l = crate::test_lock();
        let w = WorkScope::begin();
        assert_eq!(w.summary(), "no work");
        redraw(RedrawReason::Resize);
        assert_eq!(w.summary(), "redraw.resize=1");
    }
}
