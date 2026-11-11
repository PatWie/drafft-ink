//! The hook the scenario drivers use to put a generated scene on screen (`docs/performance.md` section 5.3).
//!
//! A driver (the desktop launcher, the web page API) builds a [`SceneInstall`] and sends it with a
//! [`SceneLoader`]. The app swaps the active tab's document for the scene at its next frame, as the test rig does
//! (`Harness::load_generated`), and points the camera at it. This is a measurement tool, not a user action: it is
//! not a command and it leaves no history.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use drafftink_gpu::RedrawReason;
use drafftink_scene::Document;

use crate::platform::WakeCell;

/// Where the camera looks after a scene is installed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Look {
    /// This world point at the centre of the canvas.
    Centre([f64; 2]),
    /// This world point at `margin` canvas points from the top left corner.
    Corner {
        /// The world point.
        at: [f64; 2],
        /// Distance from the corner of the canvas, in points.
        margin: f64,
    },
}

/// A document to show and where to look at it.
#[derive(Debug, Clone)]
pub struct SceneInstall {
    /// The new content of the active tab.
    pub doc: Document,
    /// Where the camera looks.
    pub look: Look,
    /// Screen pixels per world unit.
    pub zoom: f64,
    /// Ids to select (replacing the selection) once the scene is in.
    pub select: Vec<String>,
}

#[cfg(feature = "scenarios")]
pub use scenarios::scene_for;

/// The scenes of the scenario drivers (`docs/performance.md` section 4.1), built from `drafftink_scene::scenegen`.
#[cfg(feature = "scenarios")]
mod scenarios {
    use drafftink_profile::{ScenarioKey, ScenarioKind};
    use drafftink_scene::scenegen::{self, CELL, GenError};

    use super::{Look, SceneInstall};

    /// The seed of every scenario scene.
    const SEED: u64 = 1;

    /// Elements the `drag` scenario moves.
    const DRAG_SELECTION: usize = 100;

    /// The generated scene a scenario runs on, with its camera. `None` for a key without a scene (`idle`).
    ///
    /// Most scenarios look at the middle of the grid at zoom 1. `drag` zooms out to a half and puts the first
    /// cell 150 points from the top left corner, clear of the toolbars. World point (20, 20) lies inside the filled
    /// rectangle `e0` and so on screen at (190, 190): the driver grabs it there.
    pub fn scene_for(key: &ScenarioKey) -> Result<Option<SceneInstall>, GenError> {
        let Some(size) = key.size else { return Ok(None) };
        let count = usize::try_from(size).map_err(|_| GenError(format!("{size} elements do not fit in memory")))?;
        let doc = scenegen::generate(count, SEED)?;
        let middle = scenegen::columns(count) as f64 * CELL / 2.0;
        let (look, zoom, select) = match key.kind {
            ScenarioKind::Drag => {
                let first_hundred = (0..count.min(DRAG_SELECTION)).map(|i| format!("e{i}")).collect();
                (Look::Corner { at: [-60.0, -60.0], margin: 150.0 }, 0.5, first_hundred)
            }
            _ => (Look::Centre([middle, middle]), 1.0, Vec::new()),
        };
        Ok(Some(SceneInstall { doc, look, zoom, select }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn idle_has_no_scene_and_a_sized_scenario_has_the_requested_count() {
            assert!(scene_for(&"idle".parse().unwrap()).unwrap().is_none());
            let s = scene_for(&"pan@1k".parse().unwrap()).unwrap().unwrap();
            assert_eq!(s.doc.elements.len(), 1_000);
            assert_eq!(s.zoom, 1.0);
        }

        #[test]
        fn drag_looks_at_the_first_block_from_a_corner() {
            let s = scene_for(&"drag@1k".parse().unwrap()).unwrap().unwrap();
            assert!(matches!(s.look, Look::Corner { .. }));
            assert_eq!(s.zoom, 0.5);
            assert_eq!(s.select.len(), 100);
        }
    }
}

/// The sending end of the scene hook. Cheap to clone and `Send`.
#[derive(Clone)]
pub struct SceneLoader {
    tx: Sender<SceneInstall>,
    wake: WakeCell,
    applied: Arc<AtomicU64>,
}

impl SceneLoader {
    /// Queues `install` and wakes the loop. The scene is on screen after the frame that follows.
    pub fn load(&self, install: SceneInstall) {
        // A closed receiver means the app is gone; there is nobody to tell.
        if self.tx.send(install).is_ok() {
            self.wake.wake(RedrawReason::Load);
        }
    }

    /// How many scenes the app has installed so far. A driver waits for this to grow after [`SceneLoader::load`].
    pub fn applied(&self) -> u64 {
        self.applied.load(Ordering::Acquire)
    }
}

impl std::fmt::Debug for SceneLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneLoader").field("applied", &self.applied()).finish()
    }
}

/// The receiving end, owned by the app.
pub(crate) struct SceneSlot {
    rx: Receiver<SceneInstall>,
    applied: Arc<AtomicU64>,
}

impl SceneSlot {
    /// The newest queued scene, if any. Older ones are dropped: only the last would be seen.
    pub(crate) fn take(&self) -> Option<SceneInstall> {
        self.rx.try_iter().last()
    }

    /// Counts a scene as installed (or refused: the driver must not wait for it forever).
    pub(crate) fn done(&self) {
        self.applied.fetch_add(1, Ordering::Release);
    }
}

/// Makes the two ends of the hook.
pub(crate) fn scene_hook(wake: WakeCell) -> (SceneLoader, SceneSlot) {
    let (tx, rx) = channel();
    let applied = Arc::new(AtomicU64::new(0));
    (SceneLoader { tx, wake, applied: applied.clone() }, SceneSlot { rx, applied })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_scene_is_taken_and_every_install_is_counted() {
        let (loader, slot) = scene_hook(WakeCell::new());
        let mk = |zoom| SceneInstall { doc: Document::new(), look: Look::Centre([0.0, 0.0]), zoom, select: Vec::new() };
        loader.load(mk(1.0));
        loader.load(mk(2.0));
        assert_eq!(slot.take().map(|s| s.zoom), Some(2.0));
        assert!(slot.take().is_none());
        assert_eq!(loader.applied(), 0);
        slot.done();
        assert_eq!(loader.applied(), 1);
    }
}
