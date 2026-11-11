//! The state of a stage and the rule for calls that arrive while a listener runs.
//!
//! A listener runs inside `handleInput`, with the stage mutably borrowed. Konva lets a listener change the scene, so
//! this does too: a call made then is queued and runs right after the listener's event has been delivered. Reads
//! cannot be answered at that moment and throw an error that says so.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use super::convert::{console_error, fail};
use crate::core::Core;
use crate::error::ApiError;
use crate::target::Renderer;

/// Everything a stage mutates.
pub(super) struct State {
    pub(super) core: Core,
    pub(super) canvas: HtmlCanvasElement,
    /// `None` while an export has it, after `destroy` and after a lost device.
    pub(super) renderer: Option<Renderer>,
    pub(super) frames: u64,
}

impl State {
    /// The canvas size in device pixels.
    pub(super) fn device_size(&self) -> [u32; 2] {
        let vp = self.core.viewport();
        let px = |css: f64| (css * vp.dpr()).round().clamp(1.0, f64::from(u32::MAX)) as u32;
        [px(vp.size().width), px(vp.size().height)]
    }

    /// Draws a frame if one is due. True when pixels were presented.
    pub(super) fn draw(&mut self) -> Result<bool, ApiError> {
        let size = self.device_size();
        let Some(renderer) = self.renderer.as_mut() else { return Ok(false) };
        if renderer.lost().is_some() {
            return Ok(false);
        }
        let Some((scene, base)) = self.core.build_frame() else { return Ok(false) };
        if renderer.draw(scene, base, size)? {
            self.core.frame_done();
            self.frames += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

type Deferred = Box<dyn FnOnce(&mut State) -> Result<(), ApiError>>;

/// The shared part of a [`super::Stage`].
pub(super) struct Shared {
    state: RefCell<State>,
    deferred: RefCell<VecDeque<Deferred>>,
    late_ids: Cell<u64>,
    handles: Cell<u32>,
}

const BUSY: &str = "the stage is busy running a listener; read it after the listener returns (changes made in a listener are applied when it returns)";

impl Shared {
    pub(super) fn new(state: State) -> Self {
        Self { state: RefCell::new(state), deferred: RefCell::default(), late_ids: Cell::new(0), handles: Cell::new(0) }
    }

    /// Runs a read.
    pub(super) fn read<R>(&self, f: impl FnOnce(&State) -> Result<R, ApiError>) -> Result<R, JsError> {
        let state = self.state.try_borrow().map_err(|_| fail(BUSY))?;
        f(&state).map_err(fail)
    }

    /// Runs a call now, or throws when the stage is busy. For calls that return something from the state.
    pub(super) fn write<R>(&self, f: impl FnOnce(&mut State) -> Result<R, ApiError>) -> Result<R, JsError> {
        let mut state = self.state.try_borrow_mut().map_err(|_| fail(BUSY))?;
        f(&mut state).map_err(fail)
    }

    /// Runs a change now, or after the running listener when the stage is busy.
    pub(super) fn mutate(&self, f: impl FnOnce(&mut State) -> Result<(), ApiError> + 'static) -> Result<(), JsError> {
        match self.state.try_borrow_mut() {
            Ok(mut state) => f(&mut state).map_err(fail),
            Err(_) => {
                self.deferred.borrow_mut().push_back(Box::new(f));
                Ok(())
            }
        }
    }

    /// Runs the changes that were queued while a listener ran. A change may fire listeners that queue more, so this
    /// repeats up to a limit.
    pub(super) fn flush(&self) {
        for _ in 0..64 {
            let Some(next) = self.deferred.borrow_mut().pop_front() else { return };
            match self.state.try_borrow_mut() {
                Ok(mut state) => {
                    if let Err(e) = next(&mut state) {
                        console_error(&format!("a change made in a listener failed: {e}"));
                    }
                }
                Err(_) => {
                    self.deferred.borrow_mut().push_front(next);
                    return;
                }
            }
        }
        console_error("listeners keep changing the stage from inside listeners; the rest is dropped");
        self.deferred.borrow_mut().clear();
    }

    /// An element id for a shape added while the stage is busy.
    pub(super) fn fresh_id(&self) -> String {
        match self.state.try_borrow_mut() {
            Ok(mut state) => state.core.new_id(),
            Err(_) => {
                self.late_ids.set(self.late_ids.get() + 1);
                format!("late{}", self.late_ids.get())
            }
        }
    }

    /// The next listener handle.
    pub(super) fn next_handle(&self) -> u32 {
        self.handles.set(self.handles.get() + 1);
        self.handles.get()
    }

    /// Direct access, for the async paths that must not hold the borrow across an `await`.
    pub(super) fn cell(&self) -> &RefCell<State> {
        &self.state
    }
}
