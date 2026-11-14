//! What the frame loop has asked for and not finished yet, for the tests that wait for a quiet app.
//!
//! [`frames_drawn`](crate::frames_drawn) counts the frames the CPU has finished. That is not enough to know that the
//! screen shows the latest state: a command asks for a frame and the browser draws it at its next animation frame
//! (so the request is *pending*), and the GPU executes a submitted frame after the CPU is done with it (so it is
//! *in flight*). On a CPU adapter (SwiftShader) both take seconds, and the page's compositor waits for the GPU, so a
//! test that sees the frame counter stand still cannot tell "the app is idle" from "the app is waiting". These two
//! numbers can: when no frame is pending and none is in flight, the picture on screen is the document.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static REDRAW_PENDING: AtomicBool = AtomicBool::new(false);
static GPU_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);

/// True while a frame was asked for now and has not been drawn yet. A delayed request (a timer, the caret) is not
/// pending until its time has come, and nothing is pending while the window is hidden.
pub fn redraw_pending() -> bool {
    REDRAW_PENDING.load(Ordering::Relaxed)
}

/// How many submitted frames the GPU has not finished. Always 0 on the WebGL2 back end, whose calls are in order and
/// have no completion to wait for.
pub fn gpu_in_flight() -> u64 {
    GPU_IN_FLIGHT.load(Ordering::Relaxed)
}

pub(crate) fn set_redraw_pending(pending: bool) {
    REDRAW_PENDING.store(pending, Ordering::Relaxed);
}

/// Counts a frame that was just submitted on `queue` until the GPU has executed it.
#[cfg(feature = "wgpu")]
pub(crate) fn gpu_frame_submitted(queue: &wgpu::Queue) {
    GPU_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    queue.on_submitted_work_done(|| {
        // A device that was lost and replaced is reset to 0 first, so a late answer must not wrap around.
        let _ = GPU_IN_FLIGHT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    });
}

/// Forgets the frames of a device that is gone: their completion never comes.
pub(crate) fn gpu_frames_forgotten() {
    GPU_IN_FLIGHT.store(0, Ordering::Relaxed);
}
