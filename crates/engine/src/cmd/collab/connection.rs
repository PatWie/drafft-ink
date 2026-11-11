//! Starting and ending a room: the part of the commands that builds or drops the client.

use std::cell::Cell;
use std::rc::Rc;

use drafftink_collab::{AuthToken, CollabClient, CollabConfig, CollabSession, ConfigError, ServerUrl};
use drafftink_protocol::{ClientId, UserInfo};
use drafftink_scene::{Color, SceneRead};
use serde_json::Value;

use super::address::RoomName;
use super::poll::finish_import;
use super::state::{CollabState, GatedTransport, Live, stage_threshold};
use crate::cmd::tab::RoomBinding;
use crate::ports::PortError;
use crate::refusal::Refusal;
use crate::session::Session;
use crate::state::{Observed, TabId};
use crate::store::TabDoc;
use drafftink_protocol::MAX_USER_NAME_BYTES;

/// What to do with the content a tab already has when it joins a room.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnLocal {
    /// Refuse with `local_content`.
    #[default]
    Refuse,
    /// Keep the content and merge it into the room.
    Merge,
    /// Replace the content with the room's.
    Discard,
}

/// Why a room could not be started.
#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error("the network port failed: {0}")]
    Port(#[from] PortError),
    #[error("the display name or colour was refused: {0}")]
    Config(#[from] ConfigError),
}

/// The name and colour from the settings, as they go on the wire. A name is cut to the protocol's byte limit
/// on a character boundary (the setting allows 40 characters, which can be more bytes).
pub(super) fn user_from_settings(s: &Session) -> UserInfo {
    let name = s.settings.get("collab.user_name").and_then(Value::as_str).unwrap_or_default();
    let mut fitted = String::new();
    for c in name.chars() {
        if fitted.len() + c.len_utf8() > MAX_USER_NAME_BYTES {
            break;
        }
        fitted.push(c);
    }
    let color = s
        .settings
        .get("collab.user_color")
        .and_then(Value::as_str)
        .and_then(|t| Color::parse(t).ok())
        .map_or(0x0063_66f1, |c| u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b));
    UserInfo { name: fitted, color }
}

/// True when the active tab holds a live element.
pub(super) fn has_content(s: &Session) -> bool {
    s.store().iter().next().is_some()
}

/// Creates the client for `room` on the active tab and starts it. Everything that can fail comes before the
/// first change, so a refusal leaves the session as it was.
pub(super) fn start(
    s: &mut Session,
    server: ServerUrl,
    token: Option<AuthToken>,
    room: &RoomName,
    on_local: OnLocal,
) -> Result<(), Refusal> {
    let id = s.cmd();
    let user = user_from_settings(s);
    let (transport, client) = prepare(s, server, token.clone(), room, on_local, user).map_err(|e| match e {
        StartError::Port(p) => Refusal::io_failed(id, "open a connection", "the server", &p.to_string()),
        StartError::Config(c) => Refusal::internal(id, c.to_string()),
    })?;
    let tab = s.tab().id;
    if on_local == OnLocal::Discard {
        discard_local(s);
    }
    let now = s.ports.clock.mono_ms();
    let gate = Rc::new(Cell::new(Default::default()));
    let mut session = CollabSession::new(client, GatedTransport::new(transport, Rc::clone(&gate)));
    session.client_mut().start(now);
    let seen_writes = s.tab().doc.local_writes();
    let live = Live {
        session,
        tab,
        room: room.id().clone(),
        gate,
        seen_writes,
        synced_once: false,
        announced_hold: false,
        follow_view: None,
    };
    s.tab_mut().ext.set(RoomBinding(Some(room.to_string())));
    // A room that is big enough is decoded in slices by `collab_poll`.
    s.tab_mut().doc.replicated_mut().set_stage_import_bytes(stage_threshold());
    s.ext.with::<CollabState, _>(|c| {
        c.server = None;
        c.token = token;
        c.live = Some(live);
    });
    s.collab_poll();
    Ok(())
}

fn prepare(
    s: &mut Session,
    server: ServerUrl,
    auth_token: Option<AuthToken>,
    room: &RoomName,
    on_local: OnLocal,
    user: UserInfo,
) -> Result<(Box<dyn drafftink_collab::Transport>, CollabClient), StartError> {
    let Some(net) = s.ports.net.as_mut() else {
        return Err(PortError::Unsupported("no network port".to_owned()).into());
    };
    let transport = net.open_transport()?;
    let client_bits = net.random();
    let jitter_bits = net.random();
    let cfg = CollabConfig {
        url: server,
        room: room.id().clone(),
        client_id: ClientId::from_bytes(client_bits.to_be_bytes()),
        user: Some(user),
        auth_token,
        jitter_seed: u64::try_from(jitter_bits & u128::from(u64::MAX)).unwrap_or(0),
        replace_when_trimmed: on_local == OnLocal::Discard,
    };
    Ok((transport, CollabClient::new(cfg)?))
}

/// Replaces the active tab's content with an empty document and clears its history.
fn discard_local(s: &mut Session) {
    let t = s.tab_mut();
    t.doc = TabDoc::new();
    t.observed = Observed::default();
    t.saved_revision = t.doc.revision();
}

/// Ends the running room: sends what is still waiting, says goodbye, closes, and unbinds the tab. Returns the
/// room and tab it left. The chosen server stays unless `forget_server`.
pub(super) fn leave(s: &mut Session, forget_server: bool) -> Option<(String, TabId)> {
    let now = s.ports.clock.mono_ms();
    let live = s.ext.with::<CollabState, _>(|c| {
        if forget_server {
            c.server = None;
            c.token = None;
        }
        c.live.take()
    });
    let mut live = live?;
    let room = live.room.to_string();
    let tab = live.tab;
    finish_import(s, tab);
    if let Some(t) = s.tab_by_id_mut(tab) {
        t.doc.replicated_mut().set_stage_import_bytes(usize::MAX);
        live.session.client_mut().leave(now, t.doc.replicated_mut());
        // The goodbye and the close are outputs of the client; the pump hands them to the transport.
        live.session.pump(now, t.doc.replicated_mut());
        t.ext.set(RoomBinding(None));
    }
    if !forget_server {
        // The server stays chosen, so the next `collab.join_room` needs no `collab.connect`.
        let url = live.session.client().config().url.clone();
        let token = live.session.client().config().auth_token.clone();
        s.ext.with::<CollabState, _>(|c| {
            c.server = Some(url);
            c.token = token;
        });
    }
    Some((room, tab))
}
