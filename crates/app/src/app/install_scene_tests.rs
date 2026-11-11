//! `App::install_scene` (docs/exceptions.md N4): a scenario driver's scene replaces the active document without
//! a command, then the camera and the selection go through commands.

use drafftink_scene::{Document, SceneRead};

use super::*;
use crate::platform::{NoHost, Platform, SharedClipboard};

#[test]
#[allow(non_snake_case)]
fn exception_N4_a_scenario_scene_replaces_the_document() {
    let fakes = drafftink_engine::ports::fake::Fakes::default();
    let platform = Platform {
        ports: fakes.ports(),
        host: Box::new(NoHost),
        wake: WakeCell::new(),
        clipboard: SharedClipboard::new(),
        typesetter: drafftink_engine::services::LazyTypesetter::shared(),
        bridge: None,
        mcp_http: None,
        is_mac: false,
    };
    let mut app = App::new(platform, AppConfig::default());
    let loader = app.scene_loader();
    let before = app.session.tab().doc.undo_depth();
    loader.load(crate::perf::SceneInstall {
        doc: Document::new(),
        look: crate::perf::Look::Centre([10.0, 20.0]),
        zoom: 2.0,
        select: Vec::new(),
    });
    app.install_scene(Size::new(800.0, 600.0));
    assert_eq!(app.session.store().iter().count(), 0, "the new document is installed");
    assert_eq!(app.session.tab().doc.undo_depth(), before, "no undo step");
    assert_eq!(loader.applied(), 1, "the driver sees the install");
    assert_eq!(app.session.tab().view.zoom, 2.0, "the camera went through a command");
}
