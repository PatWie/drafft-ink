//! The address bar follows the encrypted room of the active tab: the link goes in when the room is created or
//! joined, and out when it is left.

use std::cell::RefCell;
use std::rc::Rc;

use drafftink_engine::ports::fake::Fakes;
use serde_json::json;

use super::*;
use crate::platform::{Host, HostJob, Inbox, Platform, SharedClipboard};

/// A host that records what it was asked to show in the address bar.
#[derive(Clone, Default)]
struct BarHost(Rc<RefCell<Vec<Option<String>>>>);

impl Host for BarHost {
    fn start(&mut self, _job: HostJob, _reply: Inbox) {}

    fn set_location_fragment(&mut self, fragment: Option<&str>) {
        self.0.borrow_mut().push(fragment.map(str::to_owned));
    }
}

fn app(fakes: &Fakes, host: &BarHost) -> App {
    let platform = Platform {
        ports: fakes.ports(),
        host: Box::new(host.clone()),
        wake: WakeCell::new(),
        clipboard: SharedClipboard::new(),
        typesetter: drafftink_engine::services::LazyTypesetter::shared(),
        bridge: None,
        mcp_http: None,
        is_mac: false,
    };
    App::new(platform, AppConfig::default())
}

#[test]
fn creating_a_room_puts_its_link_in_the_address_bar_and_leaving_takes_it_out() {
    let fakes = Fakes::new();
    let host = BarHost::default();
    let mut app = app(&fakes, &host);
    app.sync_location();
    assert!(host.0.borrow().is_empty(), "no room, nothing to show");

    app.session.execute("tab.new", json!({})).expect("an empty tab");
    app.session.execute("collab.connect", json!({ "url": "ws://localhost:3030/ws" })).expect("a server");
    app.session.execute("collab.join_room", json!({})).expect("a new encrypted room");
    app.sync_location();
    app.sync_location();
    let link =
        app.session.execute("collab.share_link", json!({ "copy": false, "reveal": true })).expect("the link")["link"]
            .as_str()
            .expect("text")
            .to_owned();
    let shown = host.0.borrow().clone();
    assert_eq!(shown.len(), 1, "told once, not every frame: {shown:?}");
    let fragment = shown[0].clone().expect("a fragment");
    assert!(fragment.starts_with("#room=") && fragment.contains(','), "the id and the key: {fragment}");
    assert!(link.ends_with(&fragment) || link.contains(&fragment), "the same room as the invite link");

    app.session.execute("collab.leave_room", json!({})).expect("leave");
    app.sync_location();
    assert_eq!(host.0.borrow().last(), Some(&None), "the fragment is removed");
    assert_eq!(host.0.borrow().len(), 2);
}

#[test]
fn a_plain_room_has_no_key_so_the_address_bar_stays_alone() {
    let fakes = Fakes::new();
    let host = BarHost::default();
    let mut app = app(&fakes, &host);
    app.session.execute("tab.new", json!({})).expect("an empty tab");
    app.session.execute("collab.connect", json!({ "url": "ws://localhost:3030/ws" })).expect("a server");
    app.session.execute("collab.join_room", json!({ "room": "team" })).expect("a plain room");
    app.sync_location();
    assert!(host.0.borrow().is_empty());
}

#[test]
fn the_dialog_gets_the_whole_invite_link_of_a_private_room_and_only_while_it_is_open() {
    let fakes = Fakes::new();
    let host = BarHost::default();
    let mut app = app(&fakes, &host);
    app.session.execute("tab.new", json!({})).expect("an empty tab");
    app.session.execute("collab.connect", json!({ "url": "ws://localhost:3030/ws" })).expect("a server");
    app.session.execute("collab.join_room", json!({})).expect("a new encrypted room");
    let closed = collab_view_of(&mut app.session, false);
    assert!(closed.e2e && closed.invite_link.is_none(), "the key is not read while the dialog is closed");
    let open = collab_view_of(&mut app.session, true);
    let link = open.invite_link.expect("a link");
    assert!(link.starts_with("https://drafft.ink/#room=") && link.contains(','), "the id and the key: {link}");
    assert!(link.ends_with("&server=ws://localhost:3030/ws"), "a bare local server is named in the link: {link}");
    let ring = serde_json::to_string(&app.session.ring().recent(1000, false, None)).unwrap();
    assert!(!ring.contains(&link) && !ring.contains("share_link"), "the read leaves no ring entry: {ring}");
}
