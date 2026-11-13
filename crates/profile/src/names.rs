//! The fixed vocabularies: counters, gauges, redraw reasons and frame phases.
//!
//! Each is a fieldless enum with a stable snake_case `name()`. The names are the keys in the
//! `PerfStats` JSON and in `perf/budgets.json`, so renaming one is a schema change
//! (see `docs/performance.md`).

macro_rules! name_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $var:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $name {
            $( $(#[$vmeta])* $var ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[ $( $name::$var ),+ ];
            /// Number of variants.
            pub const COUNT: usize = Self::ALL.len();

            /// The stable snake_case name used in JSON and budgets.
            pub const fn name(self) -> &'static str {
                match self { $( $name::$var => $text ),+ }
            }

            /// Position in [`Self::ALL`].
            pub const fn index(self) -> usize {
                self as usize
            }

            /// The variant called `name`, if any.
            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.name() == name)
            }
        }
    };
}

name_enum! {
    /// Monotonic work counters. Tests read them through `WorkScope`; the app exports them in
    /// `PerfStats.counters`.
    pub enum Counter {
        /// A per-element cache (encoded fragment, sketch path, text layout) served an entry.
        CacheHit => "cache_hit",
        /// A per-element cache had no valid entry and the work ran.
        CacheMiss => "cache_miss",
        /// An element was encoded into the render scene (fragment built or replayed).
        ElementsEncoded => "elements_encoded",
        /// An element was skipped because it is outside the viewport.
        ElementsCulled => "elements_culled",
        /// A command ran through the registry.
        Commands => "commands",
        /// A point or area hit test ran.
        HitTests => "hit_tests",
        /// A spatial index query ran.
        SpatialQueries => "spatial_queries",
        /// A text or math layout ran (not served from cache).
        Relayouts => "relayouts",
        /// A hand-drawn path was generated (not served from cache).
        SketchRegens => "sketch_regens",
        /// The whole render scene was assembled.
        SceneBuilds => "scene_builds",
        /// A GPU submission was made.
        GpuSubmits => "gpu_submits",
        /// A remote or local sync operation was applied to the document.
        SyncOpsApplied => "sync_ops_applied",
        /// A redraw request was dropped by the idle gate because nothing was dirty.
        RedrawsSkipped => "redraws_skipped",
        /// An element was visited by a pass over the whole scene (a scan, not an index lookup). An
        /// operation whose cost must not grow with the scene reports 0 here.
        ElementsScanned => "elements_scanned",
    }
}

name_enum! {
    /// Last-value readings (not monotonic).
    pub enum Gauge {
        /// Elements in the document.
        ElementsTotal => "elements_total",
        /// Elements that intersect the viewport.
        ElementsVisible => "elements_visible",
        /// Entries held by the render caches.
        CacheEntries => "cache_entries",
    }
}

name_enum! {
    /// Why a redraw was requested. The idle gate passes one of these for every redraw, so a
    /// frame can always be explained. `Other` is for code that has no better tag and is
    /// reviewed in `docs/performance.md`.
    pub enum RedrawReason {
        /// The first frame after launch.
        Startup => "startup",
        /// A command changed the document or the editor state.
        Command => "command",
        /// A live gesture update (pan, zoom, drag, resize, draw).
        Gesture => "gesture",
        /// The hovered element or handle changed.
        HoverChange => "hover_change",
        /// The window or canvas size or scale factor changed.
        Resize => "resize",
        /// egui asked to repaint for its own state (widget interaction, animation).
        Ui => "ui",
        /// A `request_repaint_after` deadline arrived.
        Timer => "timer",
        /// The text caret blinked.
        CaretBlink => "caret_blink",
        /// A running animation needs its next frame.
        Animation => "animation",
        /// A collaborator's edit or presence update arrived.
        Collab => "collab",
        /// Autosave finished and the UI shows it.
        Autosave => "autosave",
        /// An asynchronous load, paste, drop or image decode finished.
        Load => "load",
        /// The surface was lost, resized by the OS, or became visible again.
        Surface => "surface",
        /// The debug panel refreshes its numbers (twice a second while it is open).
        DebugPanel => "debug_panel",
        /// No better tag exists. Every use is a review item.
        Other => "other",
    }
}

/// A set of [`RedrawReason`]s. A frame can be requested for several reasons at once; the set says which.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct RedrawReasons(u32);

// One bit per reason.
const _: () = assert!(RedrawReason::COUNT <= u32::BITS as usize);

impl RedrawReasons {
    /// No reason: a frame with this set is unexplained.
    pub const NONE: Self = Self(0);

    const fn bit(reason: RedrawReason) -> u32 {
        1 << reason.index()
    }

    /// A set of one reason.
    pub const fn of(reason: RedrawReason) -> Self {
        Self(Self::bit(reason))
    }

    /// Adds `reason`.
    pub fn insert(&mut self, reason: RedrawReason) {
        self.0 |= Self::bit(reason);
    }

    /// True when `reason` is in the set.
    pub const fn contains(self, reason: RedrawReason) -> bool {
        self.0 & Self::bit(reason) != 0
    }

    /// True when the set has no reason.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The reasons in the set, in declaration order.
    pub fn iter(self) -> impl Iterator<Item = RedrawReason> {
        RedrawReason::ALL.iter().copied().filter(move |r| self.contains(*r))
    }

    /// The set as raw bits, for an atomic.
    pub(crate) const fn bits(self) -> u32 {
        self.0
    }

    /// The set from raw bits taken out of an atomic.
    pub(crate) const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
}

impl From<RedrawReason> for RedrawReasons {
    fn from(reason: RedrawReason) -> Self {
        Self::of(reason)
    }
}

impl FromIterator<RedrawReason> for RedrawReasons {
    fn from_iter<I: IntoIterator<Item = RedrawReason>>(iter: I) -> Self {
        let mut set = Self::NONE;
        for reason in iter {
            set.insert(reason);
        }
        set
    }
}

impl std::fmt::Debug for RedrawReasons {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

name_enum! {
    /// The phases of one frame. A phase is timed with `profile::phase(Phase::X)`; the part of
    /// a frame outside every phase is reported as `other`.
    pub enum Phase {
        /// Event handling and gesture updates.
        Input => "input",
        /// Command execution.
        Commands => "commands",
        /// Text, math and sketch layout done during the frame.
        Layout => "layout",
        /// Building the render scene.
        Encode => "encode",
        /// Handing the scene to the GPU and presenting.
        GpuSubmit => "gpu_submit",
        /// egui run and tessellation.
        Ui => "ui",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_snake_case_and_round_trip() {
        fn check<T: Copy + PartialEq + std::fmt::Debug>(
            all: &[T],
            name: fn(T) -> &'static str,
            from: fn(&str) -> Option<T>,
        ) {
            for (i, v) in all.iter().enumerate() {
                let n = name(*v);
                assert!(!n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "bad name {n}");
                assert_eq!(from(n), Some(*v));
                assert!(all.iter().skip(i + 1).all(|o| name(*o) != n), "duplicate name {n}");
            }
        }
        check(Counter::ALL, Counter::name, Counter::from_name);
        check(Gauge::ALL, Gauge::name, Gauge::from_name);
        check(RedrawReason::ALL, RedrawReason::name, RedrawReason::from_name);
        check(Phase::ALL, Phase::name, Phase::from_name);
    }

    #[test]
    fn index_matches_position() {
        for (i, c) in Counter::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
        }
        assert_eq!(Phase::COUNT, 6);
    }

    #[test]
    fn a_reason_set_holds_what_was_put_in_and_iterates_in_declaration_order() {
        let mut set = RedrawReasons::NONE;
        assert!(set.is_empty());
        set.insert(RedrawReason::Collab);
        set.insert(RedrawReason::Command);
        set.insert(RedrawReason::Collab);
        assert!(!set.is_empty());
        assert!(set.contains(RedrawReason::Collab) && set.contains(RedrawReason::Command));
        assert!(!set.contains(RedrawReason::Timer));
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![RedrawReason::Command, RedrawReason::Collab]);
        assert_eq!(set, RedrawReasons::from_iter([RedrawReason::Command, RedrawReason::Collab]));
        // Every reason has its own place in the set.
        for r in RedrawReason::ALL {
            assert_eq!(RedrawReasons::of(*r).iter().collect::<Vec<_>>(), vec![*r]);
        }
        assert_eq!(format!("{set:?}"), "{Command, Collab}");
    }
}
