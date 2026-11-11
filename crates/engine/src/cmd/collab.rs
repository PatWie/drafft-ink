//! `collab`: rooms and presence (docs/commands.md §2.16).
//!
//! The commands only change the inputs of the collab client; they never touch a socket. The client and its
//! transport live in the session ([`state`]), the shell supplies the transport through the [`Net`] port, and
//! [`Session::collab_poll`] moves the bytes. Remote edits reach the document through the same path as
//! [`Session::apply_remote`] and are never undoable by this peer. Local commits, including undo and redo, are
//! handed to the client by `collab_poll` (it compares `TabDoc::local_writes`), so every path that writes the
//! document is sent.
//!
//! Layout: `address` (server address, room name, room link), `connection` (start and leave a room), `state`
//! (what the session keeps), `poll` (driving the client), `view` (what the UI reads), `fake_server` (an
//! in-memory server for tests).

mod address;
mod connection;
pub mod fake_server;
mod poll;
mod state;
mod token;
mod view;

use drafftink_collab::AuthToken;
use drafftink_protocol::Point as WirePoint;
use drafftink_scene::Color;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use self::address::{AddressError, RoomName, resolve_server, room_link};
use self::connection::{has_content, leave, user_from_settings};
use self::state::CollabState;
use self::token::TokenArg;
use crate::args;
use crate::cmd::settings;
use crate::cmd::tab::RoomBinding;
use crate::ports::PortKind;
use crate::refusal::{Refusal, echo};
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, NoArgs, PortSet, Redraw, RingMode, TextGate, Typed};

pub use self::address::RoomLink;
pub use self::connection::OnLocal;
pub use self::poll::CollabPoll;
pub use self::state::CollabNotice;
pub use self::view::{CollabView, ConnectionState, LaserDot, Loading, PeerView, TrailDot};

/// Most points one `collab.laser` call takes.
const MAX_LASER_POINTS: usize = 1024;
/// The range of a laser fade, in milliseconds.
const FADE_MS: std::ops::RangeInclusive<u64> = 1..=10_000;

use crate::args::Pt;

// ---- reading the state ----

/// The address of the server this session is connected to or has chosen.
fn target_url(s: &Session) -> Option<String> {
    let st = s.ext.get::<CollabState>()?;
    match (&st.live, &st.server) {
        (Some(live), _) => Some(live.session.client().config().url.to_string()),
        (None, Some(server)) => Some(server.to_string()),
        (None, None) => None,
    }
}

/// The room the session is in.
fn live_room(s: &Session) -> Option<String> {
    s.ext.get::<CollabState>()?.live.as_ref().map(|l| l.room.to_string())
}

fn already_connected(id: &str, url: &str) -> Refusal {
    Refusal::custom(
        "already_connected",
        id,
        format!("Already connected to `{url}`. Disconnect first with `collab.disconnect`."),
    )
    .suggesting("collab.disconnect", None)
}

fn already_in_room(id: &str, room: &str) -> Refusal {
    Refusal::custom("already_in_room", id, format!("Already in room `{room}`. Leave it first with `collab.leave_room`."))
        .suggesting("collab.leave_room", None)
}

fn not_connected(id: &str) -> Refusal {
    Refusal::custom("not_connected", id, "Not connected to a server. Connect first with `collab.connect`.")
        .suggesting("collab.connect", None)
}

/// The state check of the commands that write the whole document out: a room that is still being filled in slices
/// would give a document with missing elements.
pub(crate) fn require_loaded(s: &Session, id: &str) -> Result<(), Refusal> {
    let Some(p) = s.tab().doc.replicated().import_progress() else { return Ok(()) };
    Err(Refusal::custom(
        "room_loading",
        id,
        format!(
            "The room is still loading ({} of {} items), so `{id}` would leave elements out. Wait until it has loaded, then run `{id}` again.",
            p.done, p.total
        ),
    ))
}

fn not_in_room(id: &str) -> Refusal {
    Refusal::custom("not_in_room", id, "This tab is not in a room, so there is nothing to leave.")
}

fn require_connected(s: &Session) -> Result<(), Refusal> {
    if target_url(s).is_some() { Ok(()) } else { Err(not_connected(s.cmd())) }
}

/// The state check of `collab.connect`: the refusal that running it would give while a server is chosen.
fn require_not_connected(s: &Session) -> Result<(), Refusal> {
    match target_url(s) {
        Some(url) => Err(already_connected(s.cmd(), &url)),
        None => Ok(()),
    }
}

/// The state check of `collab.join_room`: a server must be chosen and the session must not be in a room.
fn require_connected_and_roomless(s: &Session) -> Result<(), Refusal> {
    require_connected(s)?;
    match live_room(s) {
        Some(room) => Err(already_in_room(s.cmd(), &room)),
        None => Ok(()),
    }
}

fn require_connected_for_disconnect(s: &Session) -> Result<(), Refusal> {
    if target_url(s).is_some() {
        Ok(())
    } else {
        Err(Refusal::custom("not_connected", s.cmd(), "Not connected, so there is nothing to disconnect."))
    }
}

fn require_room_tab(s: &Session) -> Result<(), Refusal> {
    match s.tab().ext.get::<RoomBinding>() {
        Some(RoomBinding(Some(_))) => Ok(()),
        _ => Err(not_in_room(s.cmd())),
    }
}

fn invalid_url(id: &str, text: &str, why: &AddressError) -> Refusal {
    Refusal::custom(
        "invalid_url",
        id,
        format!(
            "`{}` is not a valid server address. Use a ws:// or wss:// URL such as `wss://host/ws`. A bare `/ws` works only in the web app, where it means this page's server. {why}.",
            echo(text)
        ),
    )
}

fn require_net(s: &Session) -> Result<(), Refusal> {
    s.require_port(PortKind::Net, "a network connection", "Run the app in a shell that provides one, or work on this device only.")
}

fn pt(id: &str, key: &str, p: Pt) -> Result<WirePoint, Refusal> {
    Ok(WirePoint { x: args::world(id, &format!("{key}[0]"), p[0])?, y: args::world(id, &format!("{key}[1]"), p[1])? })
}

/// The token argument checked against the protocol's limits. The refusal never repeats the token.
fn checked_token(id: &str, token: Option<&TokenArg>) -> Result<Option<AuthToken>, Refusal> {
    token
        .map(|t| t.checked().map_err(|e| Refusal::bad_arg(id, "token", e.to_string())))
        .transpose()
}

// ---- collab.connect ----

/// Arguments of `collab.connect`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ConnectArgs {
    /// `ws://` or `wss://` address of the server, or a path such as `/ws` in the web app (this page's server).
    /// Omit it to use the setting `collab.server_url`.
    pub url: Option<String>,
    /// The room to join at once. Omit it to only choose the server.
    pub room: Option<String>,
    /// What to do with content this tab already has when `room` is given.
    pub on_local: OnLocal,
    /// The access token, when the server asks for one. Kept with the server for the next `collab.join_room`.
    /// It is never recorded, shown or stored.
    pub token: Option<TokenArg>,
}

/// Output of `collab.connect`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ConnectOut {
    /// True: the server is chosen or the connection started.
    pub changed: bool,
    /// `connecting` when a room was given, else `ready`.
    pub state: ConnectionState,
    /// The server address.
    pub url: String,
    /// The room, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// `#room=<id>&server=<url>`, when a room was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

fn connect(s: &mut Session, a: ConnectArgs) -> Result<ConnectOut, Refusal> {
    let id = s.cmd();
    require_net(s)?;
    let text = a.url.unwrap_or_else(|| {
        s.settings.get("collab.server_url").and_then(|v| v.as_str()).unwrap_or_default().to_owned()
    });
    let server = resolve_server(s.ports.net.as_deref(), &text).map_err(|e| invalid_url(id, &text, &e))?;
    let room = a
        .room
        .as_deref()
        .map(|r| RoomName::parse(r).map_err(|e| Refusal::bad_arg(id, "room", e.to_string())))
        .transpose()?;
    let token = checked_token(id, a.token.as_ref())?;
    if let Some(url) = target_url(s) {
        return Err(already_connected(id, &url));
    }
    let url = server.to_string();
    match room {
        None => {
            s.ext.with::<CollabState, _>(|c| {
                c.server = Some(server);
                c.token = token;
            });
            Ok(ConnectOut { changed: true, state: ConnectionState::Ready, url, room: None, link: None })
        }
        Some(room) => {
            let link = join(s, server, token, room.clone(), a.on_local)?;
            Ok(ConnectOut {
                changed: true,
                state: ConnectionState::Connecting,
                url,
                room: Some(room.to_string()),
                link: Some(link),
            })
        }
    }
}

// ---- collab.disconnect ----

/// Output of `collab.disconnect`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DisconnectOut {
    /// Always true.
    pub changed: bool,
    /// The room that was left, if the session was in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_room: Option<String>,
}

fn disconnect(s: &mut Session, _: NoArgs) -> Result<DisconnectOut, Refusal> {
    let left = leave(s, true).map(|(room, _)| room);
    s.ext.with::<CollabState, _>(|c| c.server = None);
    Ok(DisconnectOut { changed: true, left_room: left })
}

// ---- collab.join_room ----

/// Arguments of `collab.join_room`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct JoinArgs {
    /// The room: 1 to 64 characters, letters, digits, `-` and `_`. Omit it to create a random one.
    pub room: Option<String>,
    /// What to do with content this tab already has.
    pub on_local: OnLocal,
    /// The access token for this room. Omit it to use the one given to `collab.connect`. It is never recorded,
    /// shown or stored.
    pub token: Option<TokenArg>,
}

/// Output of `collab.join_room`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct JoinOut {
    /// Always true.
    pub changed: bool,
    /// The room.
    pub room: String,
    /// `#room=<id>&server=<url>`: send it to somebody to invite them.
    pub link: String,
    /// The tab bound to the room.
    pub tab: String,
}

fn join_room(s: &mut Session, a: JoinArgs) -> Result<JoinOut, Refusal> {
    let id = s.cmd();
    require_net(s)?;
    let room = match a.room.as_deref() {
        Some(r) => RoomName::parse(r).map_err(|e| Refusal::bad_arg(id, "room", e.to_string()))?,
        None => {
            let bits = s.ports.net.as_mut().map_or(0, |n| n.random());
            RoomName::generate(bits).map_err(|e| Refusal::internal(id, e.to_string()))?
        }
    };
    if let Some(current) = live_room(s) {
        return Err(already_in_room(id, &current));
    }
    let server = s
        .ext
        .get::<CollabState>()
        .and_then(|st| st.server.clone())
        .ok_or_else(|| not_connected(id))?;
    let token = match checked_token(id, a.token.as_ref())? {
        Some(t) => Some(t),
        None => s.ext.get::<CollabState>().and_then(|st| st.token.clone()),
    };
    let link = join(s, server, token, room.clone(), a.on_local)?;
    Ok(JoinOut { changed: true, room: room.to_string(), link, tab: s.tab().id.to_string() })
}

/// The refusals and the start shared by `collab.join_room` and `collab.connect` with a room. Returns the link.
fn join(
    s: &mut Session,
    server: drafftink_collab::ServerUrl,
    token: Option<AuthToken>,
    room: RoomName,
    on_local: OnLocal,
) -> Result<String, Refusal> {
    if on_local == OnLocal::Refuse && has_content(s) {
        return Err(Refusal::custom(
            "local_content",
            s.cmd(),
            format!(
                "This tab already has content, which joining would merge into room `{room}`. Pass on_local=\"merge\" to merge it, on_local=\"discard\" to replace it with the room's content, or open a new tab first with `tab.new`."
            ),
        )
        .suggesting("tab.new", None));
    }
    let link = room_link(&room, &server);
    connection::start(s, server, token, &room, on_local)?;
    Ok(link)
}

// ---- collab.leave_room ----

/// Output of `collab.leave_room`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct LeaveOut {
    /// Always true.
    pub changed: bool,
    /// The room that was left.
    pub room: String,
}

fn leave_room(s: &mut Session, _: NoArgs) -> Result<LeaveOut, Refusal> {
    let id = s.cmd();
    match leave(s, false) {
        Some((room, _)) => Ok(LeaveOut { changed: true, room }),
        // The tab says it is in a room but no client runs (the workspace was restored): just forget the room.
        None => {
            let room = match s.tab().ext.get::<RoomBinding>() {
                Some(RoomBinding(Some(r))) => r.clone(),
                _ => return Err(not_in_room(id)),
            };
            s.tab_mut().ext.set(RoomBinding(None));
            Ok(LeaveOut { changed: true, room })
        }
    }
}

// ---- collab.set_user ----

/// Arguments of `collab.set_user`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetUserArgs {
    /// The name other people see, 0 to 40 characters.
    pub name: Option<String>,
    /// The colour of your cursor, `#rrggbb`.
    pub color: Option<Color>,
}

/// Output of `collab.set_user`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetUserOut {
    /// False when the name and colour were already as asked.
    pub changed: bool,
    /// The name now in use.
    pub name: String,
    /// The colour now in use, `#rrggbb`.
    pub color: String,
}

fn set_user(s: &mut Session, a: SetUserArgs) -> Result<SetUserOut, Refusal> {
    let mut changed = false;
    if let Some(name) = &a.name {
        changed |= settings::set_internal(s, "collab.user_name", &json!(name))?;
    }
    if let Some(color) = a.color {
        changed |= settings::set_internal(s, "collab.user_color", &json!(color.to_hex()))?;
    }
    let user = user_from_settings(s);
    let out = SetUserOut { changed, name: user.name.clone(), color: view::hex(user.color) };
    s.ext.with::<CollabState, _>(|c| {
        if let Some(live) = c.live.as_mut() {
            live.session.client_mut().set_user(Some(user));
        }
    });
    s.collab_poll();
    Ok(out)
}

// ---- collab.set_pointer ----

/// Which pointer the person is using.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerTool {
    /// A plain cursor.
    #[default]
    Pointer,
    /// The laser: peers draw the recent path of the pointer as a trail.
    Laser,
}

/// Arguments of `collab.set_pointer`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetPointerArgs {
    /// The pointer in world coordinates `[x, y]`, or null when it left the canvas.
    pub point: Option<Pt>,
    /// The pointer in use. Default `pointer`.
    pub tool: PointerTool,
}

/// Output of `collab.set_pointer`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PointerOut {
    /// Always false: nothing on this screen changes.
    pub changed: bool,
    /// True when a room received the pointer.
    pub published: bool,
}

fn set_pointer(s: &mut Session, a: SetPointerArgs) -> Result<PointerOut, Refusal> {
    let id = s.cmd();
    let point = a.point.map(|p| pt(id, "point", p)).transpose()?;
    let now = s.ports.clock.mono_ms();
    let published = s.ext.with::<CollabState, _>(|c| match c.live.as_mut() {
        Some(live) => {
            let client = live.session.client_mut();
            client.set_laser(a.tool == PointerTool::Laser);
            client.set_cursor(now, point);
            true
        }
        None => false,
    });
    Ok(PointerOut { changed: false, published })
}

// ---- collab.laser ----

/// Arguments of `collab.laser`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaserArgs {
    /// Points of the trail in world coordinates `[x, y]`, oldest first. At most 1024.
    pub points: Vec<Pt>,
    /// How long a point stays visible, in milliseconds (1 to 10000).
    #[serde(default = "default_fade_ms")]
    pub fade_ms: u64,
}

fn default_fade_ms() -> u64 {
    330
}

/// Output of `collab.laser`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct LaserOut {
    /// True when points were added.
    pub changed: bool,
    /// How many points were added.
    pub points: usize,
}

fn laser(s: &mut Session, a: LaserArgs) -> Result<LaserOut, Refusal> {
    let id = s.cmd();
    if a.points.len() > MAX_LASER_POINTS {
        return Err(Refusal::limit_exceeded(id, a.points.len(), "laser points", MAX_LASER_POINTS));
    }
    if !FADE_MS.contains(&a.fade_ms) {
        return Err(Refusal::bad_arg(id, "fade_ms", "must be a whole number of milliseconds from 1 to 10000"));
    }
    let fade_ms = a.fade_ms;
    let points: Vec<WirePoint> =
        a.points.iter().map(|p| pt(id, "points", *p)).collect::<Result<_, _>>()?;
    let now = s.ports.clock.mono_ms();
    s.ext.with::<CollabState, _>(|c| {
        for p in &points {
            c.laser.push(now, fade_ms, p.x, p.y);
        }
        if let (Some(live), Some(last)) = (c.live.as_mut(), points.last()) {
            let client = live.session.client_mut();
            client.set_laser(true);
            client.set_cursor(now, Some(*last));
        }
    });
    Ok(LaserOut { changed: !points.is_empty(), points: points.len() })
}

// ---- collab.follow ----

/// Arguments of `collab.follow`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct FollowArgs {
    /// The person to follow: the id or the name `collab.status` shows. Omit it to stop following.
    pub peer: Option<String>,
}

/// Output of `collab.follow`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct FollowOut {
    /// False when following was already as asked.
    pub changed: bool,
    /// The name of the person now followed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub following: Option<String>,
}

fn follow(s: &mut Session, a: FollowArgs) -> Result<FollowOut, Refusal> {
    let id = s.cmd();
    let out = s.ext.with::<CollabState, _>(|c| {
        let live = c.live.as_mut().ok_or_else(|| not_in_room(id))?;
        let client = live.session.client_mut();
        let before = client.following();
        let Some(wanted) = a.peer.as_deref() else {
            client.stop_following();
            live.follow_view = None;
            return Ok(FollowOut { changed: before.is_some(), following: None });
        };
        let found = client
            .peers()
            .iter()
            .find(|p| p.id.to_string().eq_ignore_ascii_case(wanted) || p.display_name().eq_ignore_ascii_case(wanted))
            .map(|p| (p.id, p.display_name()));
        let Some((peer, name)) = found else {
            return Err(Refusal::custom(
                "no_such_peer",
                id,
                format!("There is nobody called `{}` in this room. List the people with `collab.status`.", echo(wanted)),
            )
            .suggesting("collab.status", None));
        };
        client.follow(peer);
        Ok(FollowOut { changed: before != Some(peer), following: Some(name) })
    })?;
    s.collab_poll();
    Ok(out)
}

// ---- collab.status ----

fn status(s: &mut Session, _: NoArgs) -> Result<CollabView, Refusal> {
    Ok(s.collab_view())
}

// ---- registration ----

const NET: Facts = Facts {
    reads_only: false,
    mutates: Mutates::COLLAB,
    ports: PortSet::NET,
    batch: Batch::Never,
    redraw: Redraw::Overlay,
    palette: true,
    ..Facts::READ_ONLY
};
const PRESENCE: Facts = Facts { ports: PortSet::NONE, ring: RingMode::Quiet, palette: false, ..NET };

/// The collab family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "collab.connect",
        "collab",
        "Connect to server",
        "Chooses the collaboration server and, when a room is given, joins it. Returns at once; the connection comes up in the background.",
        &Typed::new(connect),
        NET,
    )
    .available(require_not_connected)
    .refusals(&["invalid_url", "already_connected", "local_content"])
    .secret_args(&["token"])
    .example(r#"{"url":"wss://draw.example.com/ws","room":"team-sync"}"#),
    CommandSpec::new(
        "collab.disconnect",
        "collab",
        "Disconnect",
        "Leaves the room, closes the connection and forgets the server. The local document stays.",
        &Typed::new(disconnect),
        NET,
    )
    .available(require_connected_for_disconnect)
    .refusals(&["not_connected"]),
    CommandSpec::new(
        "collab.join_room",
        "collab",
        "Join room",
        "Binds a room to the active tab and starts syncing it. Omit the room to create a random one.",
        &Typed::new(join_room),
        Facts {
            mutates: Mutates::COLLAB.with(Mutates::DOC).with(Mutates::TABS),
            view_only: Gate::Refused,
            while_gesture: Gate::Refused,
            while_text_edit: TextGate::Commit,
            doc_write: crate::spec::DocWrite::Replaces,
            ..NET
        },
    )
    .available(require_connected_and_roomless)
    .refusals(&["not_connected", "already_in_room", "local_content"])
    .secret_args(&["token"])
    .example(r#"{"room":"team-sync","on_local":"merge"}"#),
    CommandSpec::new(
        "collab.leave_room",
        "collab",
        "Leave room",
        "Stops syncing the active tab and removes its collaborators. The local copy of the document stays.",
        &Typed::new(leave_room),
        NET,
    )
    .available(require_room_tab)
    .refusals(&["not_in_room"]),
    CommandSpec::new(
        "collab.set_user",
        "collab",
        "Set name and colour",
        "Sets the name and cursor colour other people see, stores them in the settings and updates the room.",
        &Typed::new(set_user),
        Facts { mutates: Mutates::SETTINGS, ports: PortSet::NONE, palette: false, ..NET },
    )
    .example(r##"{"name":"Ana","color":"#22c55e"}"##),
    CommandSpec::new(
        "collab.set_pointer",
        "collab",
        "Publish pointer",
        "Publishes the pointer position in world coordinates to the room, or clears it with null. Ephemeral.",
        &Typed::new(set_pointer),
        Facts { agent: false, ..PRESENCE },
    ),
    CommandSpec::new(
        "collab.laser",
        "collab",
        "Laser trail",
        "Shows a fading laser trail on this screen and to the room. Nothing is stored in the document.",
        &Typed::new(laser),
        PRESENCE,
    )
    .example(r#"{"points":[[0,0],[40,10],[90,30]],"fade_ms":330}"#),
    CommandSpec::new(
        "collab.follow",
        "collab",
        "Follow collaborator",
        "Makes the camera follow a collaborator's visible area. Omit the peer to stop following.",
        &Typed::new(follow),
        Facts { mutates: Mutates::COLLAB.with(Mutates::VIEW), ports: PortSet::NONE, palette: false, ..NET },
    )
    .available(require_room_tab)
    .refusals(&["not_in_room", "no_such_peer"]),
    CommandSpec::new(
        "collab.status",
        "collab",
        "Collab status",
        "Reports the connection, the room, the people in it with their pointers, and whether a room is still loading.",
        &Typed::new(status),
        Facts::READ_ONLY,
    ),
];

#[cfg(test)]
#[path = "collab_tests.rs"]
mod tests;
