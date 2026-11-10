//! docs/performance.md must keep up with the code: every name the profiler exports is
//! documented there, so the guide cannot silently drift from the schema.

use drafftink_profile::{Counter, Gauge, Phase, RedrawReason, SCHEMA_VERSION};

const DOC: &str = include_str!("../../../docs/performance.md");

fn documented(kind: &str, name: &str) {
    assert!(
        DOC.contains(&format!("`{name}`")),
        "docs/performance.md does not document the {kind} `{name}`"
    );
}

#[test]
fn every_counter_gauge_reason_and_phase_is_documented() {
    for c in Counter::ALL {
        documented("counter", c.name());
    }
    for g in Gauge::ALL {
        documented("gauge", g.name());
    }
    for r in RedrawReason::ALL {
        documented("redraw reason", r.name());
    }
    for p in Phase::ALL {
        documented("phase", p.name());
    }
}

#[test]
fn the_documented_schema_version_is_current() {
    assert!(
        DOC.contains(&format!("Schema version: {SCHEMA_VERSION}")),
        "docs/performance.md must say \"Schema version: {SCHEMA_VERSION}\""
    );
}
