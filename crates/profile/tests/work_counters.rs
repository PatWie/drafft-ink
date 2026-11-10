//! How other crates' tests use the profiler: assert on work done, never on time.
//! Each test runs on its own thread, so `WorkScope` sees only its own work.

use drafftink_profile::{Counter, Gauge, RedrawReason, WorkScope, count, inc, redraw, set_gauge};

/// A stand-in for the scene encoder: encodes the visible elements, culls the rest, and
/// serves repeats from a cache.
struct Encoder {
    cached: Vec<bool>,
}

impl Encoder {
    fn new(n: usize) -> Self {
        Self {
            cached: vec![false; n],
        }
    }

    fn encode(&mut self, visible: std::ops::Range<usize>) {
        for (i, c) in self.cached.iter_mut().enumerate() {
            if !visible.contains(&i) {
                inc(Counter::ElementsCulled);
            } else if *c {
                inc(Counter::CacheHit);
                inc(Counter::ElementsEncoded);
            } else {
                inc(Counter::CacheMiss);
                inc(Counter::ElementsEncoded);
                *c = true;
            }
        }
        set_gauge(Gauge::ElementsVisible, visible.len() as u64);
    }
}

#[test]
fn an_operation_encodes_exactly_n_elements() {
    let mut enc = Encoder::new(1000);
    let work = WorkScope::begin();
    enc.encode(100..110);
    assert_eq!(work.count(Counter::ElementsEncoded), 10);
    assert_eq!(work.count(Counter::ElementsCulled), 990);
    assert_eq!(work.count(Counter::CacheMiss), 10);
    assert_eq!(work.count(Counter::CacheHit), 0);
    assert_eq!(work.gauge(Gauge::ElementsVisible), 10);
}

#[test]
fn panning_over_cached_elements_encodes_from_cache() {
    let mut enc = Encoder::new(100);
    enc.encode(0..20);
    let pan = WorkScope::begin();
    enc.encode(0..20);
    assert_eq!(pan.count(Counter::CacheMiss), 0, "{}", pan.summary());
    assert_eq!(pan.count(Counter::CacheHit), 20);
}

#[test]
fn no_redraw_happened_in_this_window() {
    let work = WorkScope::begin();
    // a pointer move that changes nothing: the idle gate drops it
    inc(Counter::RedrawsSkipped);
    assert_eq!(work.redraws(), 0, "{}", work.summary());
    assert_eq!(work.count(Counter::RedrawsSkipped), 1);

    // a pointer move onto a new element: one redraw, and the reason says why
    redraw(RedrawReason::HoverChange);
    assert_eq!(work.redraw_reasons(), vec![RedrawReason::HoverChange]);
}

#[test]
fn a_failed_expectation_explains_itself() {
    let work = WorkScope::begin();
    count(Counter::Relayouts, 2);
    redraw(RedrawReason::Timer);
    assert_eq!(work.summary(), "relayouts=2 redraw.timer=1");
}
