//! Shared fixture of the UI tests: a real `Session` over fake ports, the model, the UI state and a
//! recorder for the actions each frame returns. No GPU: `egui_kittest` runs the UI headless.
#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use drafftink_engine::cmd::FAMILIES;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::Services;
use drafftink_engine::spec::CommandSpec;
use drafftink_engine::{Registry, Session};
use drafftink_ui::{Call, Model, Report, UiAction, UiState, dispatch, draw};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// Everything a test needs.
pub struct Fixture {
    /// The session the UI reads and the commands run on.
    pub session: Session,
    /// Fake ports (clipboard, storage, files, ...).
    pub fakes: Fakes,
    /// The model snapshot.
    pub model: Model,
    /// UI-only state.
    pub state: UiState,
    /// Every action the UI returned, in order, over all frames.
    pub actions: Vec<UiAction>,
    /// What the commands returned when the tests dispatched actions.
    pub report: Report,
}

impl Fixture {
    /// A fresh session with the fake text engine.
    pub fn new() -> Self {
        let fakes = Fakes::new();
        let session = Session::with_services(fakes.ports(), Services::fake());
        Self::over(session, fakes)
    }

    /// A fresh session whose registry leaves out the commands with these ids, as a build that does not
    /// have them would. Used to test the "not available in this build" behaviour of the controls without
    /// relying on a gap of the real registry.
    pub fn without(omit: &[&str]) -> Self {
        let families: Vec<&'static [CommandSpec]> =
            FAMILIES.iter().flat_map(|family| family.split(|spec| omit.contains(&spec.id))).collect();
        let fakes = Fakes::new();
        let mut session = Session::with_registry(fakes.ports(), Arc::new(Registry::from_families(&families)));
        session.services = Services::fake();
        Self::over(session, fakes)
    }

    /// Over a given session.
    pub fn over(session: Session, fakes: Fakes) -> Self {
        let mut f = Self {
            session,
            fakes,
            model: Model::new(false),
            state: UiState::new(),
            actions: Vec::new(),
            report: Report::default(),
        };
        f.refresh();
        f
    }

    /// Recaptures the model.
    pub fn refresh(&mut self) {
        self.model.invalidate();
        self.model.refresh(&mut self.session);
    }

    /// The commands among the recorded actions.
    pub fn calls(&self) -> Vec<&Call> {
        self.actions.iter().filter_map(UiAction::as_call).collect()
    }

    /// The last recorded command call.
    pub fn last_call(&self) -> Option<&Call> {
        self.calls().last().copied()
    }

    /// Forgets the recorded actions.
    pub fn clear(&mut self) {
        self.actions.clear();
    }

    /// Executes the recorded actions on the session as the app would, then clears them. The report is
    /// returned and also kept in `self.report`.
    pub fn dispatch(&mut self) -> Report {
        let actions = std::mem::take(&mut self.actions);
        let r = dispatch(&mut self.session, &mut self.state, actions, 0.0);
        self.refresh();
        self.report = r.clone();
        r
    }
}

/// A harness that draws the UI of the fixture each frame and appends the returned actions to it.
pub fn harness(fixture: Fixture) -> Harness<'static, Fixture> {
    Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, f: &mut Fixture| {
            let acts = draw(ui, &f.model, &mut f.state);
            f.actions.extend(acts);
        },
        fixture,
    )
}

/// Clicks the control with this accessible name and returns the command calls it produced.
pub fn click(h: &mut Harness<'static, Fixture>, label: &str) -> Vec<Call> {
    h.state_mut().clear();
    h.get_by_label(label).click();
    h.run();
    h.state().calls().into_iter().cloned().collect()
}
