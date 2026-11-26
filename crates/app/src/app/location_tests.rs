//! The address bar follows the encrypted room of the active tab: the link goes in when the room is created or
//! joined, and out when it is left.

use std::cell::RefCell;
use std::rc::Rc;

use drafftink_engine::ports::fake::Fakes;
use serde_json::json;

use drafftink_engine::cmd::collab::RoomLink;

use super::*;
use crate::config::StartupLink;
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
fn a_room_that_is_still_connecting_puts_nothing_in_the_address_bar() {
    let fakes = Fakes::new();
    let host = BarHost::default();
    let mut app = app(&fakes, &host);
    app.sync_location();
    assert!(host.0.borrow().is_empty(), "no room, nothing to show");

    app.session.execute("tab.new", json!({})).expect("an empty tab");
    app.session.execute("collab.connect", json!({ "url": "ws://localhost:3030/ws" })).expect("a server");
    app.session.execute("collab.join_room", json!({})).expect("a new encrypted room");
    // No relay answers: the room is bound and connecting, and it never joins.
    app.sync_location();
    app.sync_location();
    assert!(app.session.sealed_room_id().is_some(), "the tab is bound to the room");
    assert_eq!(app.session.joined_room_id(), None);
    assert!(host.0.borrow().is_empty(), "a link of a room that has not joined is not written: {:?}", host.0.borrow());

    app.session.execute("collab.leave_room", json!({})).expect("leave");
    app.sync_location();
    assert!(host.0.borrow().is_empty(), "nothing was shown, so nothing is taken out");
}

#[test]
fn the_address_bar_follows_the_room_the_session_has_joined() {
    use super::{
        AddressStep::{Clear, Keep, Show},
        address_step,
    };
    // (joined, located, opened with a link, connecting) -> what the bar is told
    assert_eq!(address_step(Some("a"), None, false, false), Show, "joined: the link goes in");
    assert_eq!(address_step(Some("a"), Some("a"), false, false), Keep, "told once, not every frame");
    assert_eq!(address_step(Some("a"), Some("b"), false, false), Show, "another room: the new link");
    assert_eq!(
        address_step(Some("a"), None, true, false),
        Show,
        "the link the page was opened with is replaced by the joined one"
    );
    assert_eq!(address_step(None, Some("a"), false, false), Clear, "left: the link goes out");
    assert_eq!(address_step(None, None, false, true), Keep, "connecting: nothing is written");
    assert_eq!(address_step(None, None, true, true), Keep, "the link that was opened stays while its room connects");
    assert_eq!(address_step(None, None, true, false), Clear, "nothing uses the link that was opened: it goes");
    assert_eq!(address_step(None, None, false, false), Keep, "nothing to show, nothing to clear");
}

#[test]
fn a_site_without_rooms_ignores_a_room_link_says_so_and_takes_it_out_of_the_address_bar() {
    let fakes = Fakes::new();
    fakes.net.set_rooms_available(false);
    let host = BarHost::default();
    let config = AppConfig {
        join: StartupLink {
            link: RoomLink { room: Some("abc,key".into()), server: Some("wss://canvas.patwie.com/ws".into()) },
        },
        ..AppConfig::default()
    };
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
    let mut app = App::new(platform, config);
    assert_eq!(
        app.notices,
        ["Rooms are not available on this site. Open the link on the server that created it."],
        "one plain notice"
    );
    assert!(fakes.net.servers().is_empty(), "nothing connected");
    assert_eq!(app.session.collab_view().state, drafftink_engine::cmd::collab::ConnectionState::Disconnected);
    app.sync_location();
    app.sync_location();
    assert_eq!(*host.0.borrow(), [None], "the room link is removed from the address once, and none is written");
    assert!(app.session.execute("collab.share_link", json!({})).is_err());
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
