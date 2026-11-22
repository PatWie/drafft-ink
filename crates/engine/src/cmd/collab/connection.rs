//! Starting and ending a room: the part of the commands that builds or drops the client.

use std::cell::Cell;
use std::rc::Rc;

use drafftink_collab::{
    AuthToken, CollabClient, CollabConfig, CollabSession, ConfigError, Resume, SealedClient, SealedConfig,
    SealedConfigError, ServerUrl,
};
use drafftink_protocol::v2::JoinMode;
use drafftink_protocol::{ClientId, UserInfo};
use drafftink_seal::RoomKey;
use drafftink_scene::{Color, SceneRead};
use serde_json::Value;

use super::address::RoomName;
use super::poll::finish_import;
use super::state::{CollabState, GatedTransport, Live, RoomRef, stage_threshold};
use crate::cmd::tab::{RoomBinding, SealedBinding, SealedRoom};
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

/// What to do when the encrypted room the link names does not exist: it expired, or the link is incomplete.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IfMissing {
    /// Stop and say so. Nothing is created: a mistyped link makes no room.
    #[default]
    Refuse,
    /// Make the room again from the copy this tab holds (it expired). The copy is the room's content from now on.
    Restore,
    /// Make an empty room under the same link.
    Create,
}

impl IfMissing {
    fn mode(self) -> JoinMode {
        if self == IfMissing::Refuse { JoinMode::Open } else { JoinMode::Create }
    }
}

/// The room a client is started for.
pub(super) enum Target {
    /// A plaintext room.
    Plain(RoomName),
    /// An encrypted room.
    Sealed {
        /// The key.
        key: RoomKey,
        /// What to do when the room does not exist.
        if_missing: IfMissing,
        /// Where an earlier connection stopped.
        resume: Resume,
    },
}

/// Why a room could not be started.
#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error("the network port failed: {0}")]
    Port(#[from] PortError),
    #[error("the display name or colour was refused: {0}")]
    Config(#[from] ConfigError),
    #[error("the saved state of the room was refused: {0}")]
    Sealed(#[from] SealedConfigError),
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

/// Creates the client for `target` on the active tab and starts it. Everything that can fail comes before the
/// first change, so a refusal leaves the session as it was.
pub(super) fn start(
    s: &mut Session,
    server: ServerUrl,
    token: Option<AuthToken>,
    target: Target,
    on_local: OnLocal,
) -> Result<(), Refusal> {
    let id = s.cmd();
    let user = user_from_settings(s);
    let (transport, client, room, binding) = prepare(s, &server, token.clone(), target, on_local, user).map_err(|e| match e {
        StartError::Port(p) => Refusal::io_failed(id, "open a connection", "the server", &p.to_string()),
        StartError::Config(c) => Refusal::internal(id, c.to_string()),
        StartError::Sealed(c) => Refusal::internal(id, c.to_string()),
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
        room: room.clone(),
        gate,
        seen_writes,
        synced_once: false,
        announced_hold: false,
        follow_view: None,
    };
    s.tab_mut().ext.set(RoomBinding(Some(room.to_string())));
    let sealed = binding.map(|(key, resume)| SealedRoom { key, server: server.to_string(), resume, joined_ms: s.now_ms() });
    s.tab_mut().ext.set(SealedBinding(sealed));
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

/// The transport and the client, the room they are for, and for an encrypted room the key and resume point that the
/// tab keeps.
type Prepared = (Box<dyn drafftink_collab::Transport>, drafftink_collab::RoomClient, RoomRef, Option<(RoomKey, Resume)>);

fn prepare(
    s: &mut Session,
    server: &ServerUrl,
    auth_token: Option<AuthToken>,
    target: Target,
    on_local: OnLocal,
    user: UserInfo,
) -> Result<Prepared, StartError> {
    let Some(net) = s.ports.net.as_mut() else {
        return Err(PortError::Unsupported("no network port".to_owned()).into());
    };
    let transport = net.open_transport()?;
    let client_bits = net.random();
    let jitter_bits = net.random();
    let client_id = ClientId::from_bytes(client_bits.to_be_bytes());
    let jitter_seed = u64::try_from(jitter_bits & u128::from(u64::MAX)).unwrap_or(0);
    let replace_when_trimmed = on_local == OnLocal::Discard;
    Ok(match target {
        Target::Plain(room) => {
            let cfg = CollabConfig {
                url: server.clone(),
                room: room.id().clone(),
                client_id,
                user: Some(user),
                auth_token,
                jitter_seed,
                replace_when_trimmed,
            };
            (transport, CollabClient::new(cfg)?.into(), RoomRef::Plain(room.id().clone()), None)
        }
        Target::Sealed { key, if_missing, resume } => {
            let kept = (RoomKey::from_bytes(*key.expose_bytes()), resume.clone());
            let id = key.room_id();
            let cfg = SealedConfig {
                url: server.clone(),
                key,
                client_id,
                user: Some(user),
                mode: if_missing.mode(),
                resume,
                jitter_seed,
                replace_when_trimmed,
            };
            (transport, SealedClient::new(cfg)?.into(), RoomRef::Sealed(id), Some(kept))
        }
    })
}

/// Replaces the active tab's content with an empty document and clears its history.
fn discard_local(s: &mut Session) {
    let t = s.tab_mut();
    t.doc = TabDoc::new();
    t.observed = Observed::default();
    t.mark_saved();
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
        // Leaving forgets the key of the tab. The document stays as a local copy.
        t.ext.set(SealedBinding(None));
    }
    if !forget_server {
        // The server stays chosen, so the next `collab.join_room` needs no `collab.connect`.
        let url = live.session.client().url().clone();
        let token = live.session.client().auth_token().cloned();
        s.ext.with::<CollabState, _>(|c| {
            c.server = Some(url);
            c.token = token;
        });
    }
    Some((room, tab))
}

impl Session {
    /// After the workspace was restored: reconnects the room of a restored encrypted tab, so that a reload comes
    /// back to the same tab and the same room. The active tab goes first. Only one room runs per session.
    pub(crate) fn resume_restored_room(&mut self) {
        if self.ports.net.is_none() {
            return;
        }
        let is_room = |t: &crate::state::Tab| t.ext.get::<SealedBinding>().is_some_and(|b| b.0.is_some());
        let active = self.active_index();
        let Some(index) = (active..self.tabs.len()).chain(0..active).find(|i| self.tabs.get(*i).is_some_and(is_room)) else {
            return;
        };
        let Some(room) = self.tabs.get(index).and_then(|t| t.ext.get::<SealedBinding>()).and_then(|b| b.0.as_ref()).map(SealedRoom::duplicate)
        else {
            return;
        };
        let policy = drafftink_collab::UrlPolicy { allow_insecure: self.ports.net.as_deref().is_some_and(crate::ports::Net::allow_insecure) };
        let Ok(server) = ServerUrl::parse(&room.server, policy) else { return };
        self.set_active_index(index);
        let target = Target::Sealed { key: room.key, if_missing: IfMissing::Refuse, resume: room.resume };
        // A room that cannot be reached shows its reason in the collaboration status; the tab stays a local copy.
        let _ = start(self, server, None, target, OnLocal::Merge);
        self.set_active_index(active);
    }
}
