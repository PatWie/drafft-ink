//! Starting and ending a room: the part of the commands that builds or drops the client.

use std::cell::Cell;
use std::rc::Rc;

use drafftink_collab::{
    AuthToken, BlobTransport, CollabClient, CollabConfig, CollabSession, ConfigError, Resume, SealedClient,
    SealedConfig, SealedConfigError, ServerUrl,
};
use drafftink_doc::FilePolicy;
use drafftink_protocol::v2::JoinMode;
use drafftink_protocol::{ClientId, UserInfo};
use drafftink_seal::RoomKey;
use drafftink_scene::{Color, SceneRead};
use serde_json::Value;

use super::access::grant_for;
use super::address::RoomName;
use super::join_check::check_local_pictures;
use super::pictures::Pictures;
use super::poll::finish_import;
use super::state::{CollabNotice, CollabState, GatedTransport, Live, RoomRef, stage_threshold};
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
    let sealed = matches!(target, Target::Sealed { .. });
    // A tab that is not yet an encrypted room's copy (a local tab) meets the room's rules for pictures first. A tab
    // that comes back to its room (restored from the workspace) is that room's copy already.
    let joins_fresh = sealed && s.tab().doc.replicated().file_policy() != FilePolicy::External;
    if joins_fresh && on_local != OnLocal::Discard {
        check_local_pictures(s)?;
    }
    let Prepared { transport, blobs, client, room, binding } =
        prepare(s, &server, token.clone(), target, on_local, user).map_err(|e| match e {
            StartError::Port(p) => Refusal::io_failed(id, "open a connection", "the server", &p.to_string()),
            StartError::Config(c) => Refusal::internal(id, c.to_string()),
            StartError::Sealed(c) => Refusal::internal(id, c.to_string()),
        })?;
    let tab = s.tab().id;
    if on_local == OnLocal::Discard {
        discard_local(s);
    }
    if joins_fresh {
        make_external(s, id)?;
    }
    let now = s.ports.clock.mono_ms();
    let gate = Rc::new(Cell::new(Default::default()));
    let mut session = CollabSession::new(client, GatedTransport::new(transport, Rc::clone(&gate)));
    match blobs {
        Some(blobs) => session.set_blobs(blobs),
        None if sealed => s.collab_notify(CollabNotice::Info(NO_PICTURES.to_owned())),
        None => {}
    }
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
        pictures: Pictures::new(sealed),
        deleted: false,
    };
    s.tab_mut().ext.set(RoomBinding(Some(room.to_string())));
    let sealed = binding.map(|(key, resume)| SealedRoom { key, server: server.to_string(), resume, joined_ms: s.now_ms() });
    s.tab_mut().ext.set(SealedBinding(sealed));
    // A room that is big enough is decoded in slices by `collab_poll`.
    s.tab_mut().doc.replicated_mut().set_stage_import_bytes(stage_threshold());
    s.ext.with::<CollabState, _>(|c| {
        c.server = None;
        c.token = token;
        c.room_deleted = None;
        c.live = Some(live);
    });
    s.collab_poll();
    Ok(())
}

/// What a shell without HTTP means for an encrypted room: it works, its pictures do not.
const NO_PICTURES: &str = "This app cannot load or send pictures here, so the pictures of this room will not show and yours will not be shared. Drawing and text work as usual.";

/// What [`prepare`] builds: the transport and the client, the room they are for, and for an encrypted room the
/// picture transport (when the shell has one) and the key and resume point that the tab keeps.
struct Prepared {
    transport: Box<dyn drafftink_collab::Transport>,
    blobs: Option<Box<dyn BlobTransport>>,
    client: drafftink_collab::RoomClient,
    room: RoomRef,
    binding: Option<(RoomKey, Resume)>,
}

/// Makes the replicated store of the active tab take pictures as files, as an encrypted room does. A tab that was
/// local gets a new replicated store built from its content: the old Loro history holds the bytes of the pictures it
/// embedded, and that history must not reach the room. The undo history is cleared by it.
fn make_external(s: &mut Session, id: &str) -> Result<(), Refusal> {
    let doc = &mut s.tab_mut().doc;
    if doc.replicated().file_policy() == FilePolicy::External {
        return Ok(());
    }
    doc.restart_replicated(FilePolicy::External).map_err(|e| Refusal::internal(id, e.to_string()))
}

fn prepare(
    s: &mut Session,
    server: &ServerUrl,
    auth_token: Option<AuthToken>,
    target: Target,
    on_local: OnLocal,
    user: UserInfo,
) -> Result<Prepared, StartError> {
    // The service token of a private server, bound to its origin. The transports send it to that origin only.
    let access = grant_for(s.ports.secrets.as_deref(), server).unwrap_or_else(|e| {
        // The token is not sent, and the server will say it is private. The person learns why.
        s.collab_notify(CollabNotice::Info(format!("The stored access token could not be read, so none is sent: {e}.")));
        None
    });
    let Some(net) = s.ports.net.as_mut() else {
        return Err(PortError::Unsupported("no network port".to_owned()).into());
    };
    let mut transport = net.open_transport()?;
    transport.set_access(access.clone());
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
            Prepared { transport, blobs: None, client: CollabClient::new(cfg)?.into(), room: RoomRef::Plain(room.id().clone()), binding: None }
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
            // A shell without HTTP still runs the room; the pictures are then missing, and `start` says so once.
            let blobs = net.open_blob_transport().ok().map(|mut b| {
                b.set_access(access);
                b
            });
            Prepared { transport, blobs, client: SealedClient::new(cfg)?.into(), room: RoomRef::Sealed(id), binding: Some(kept) }
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
            c.room_deleted = None;
        }
        c.live.take()
    });
    let mut live = live?;
    let room = live.room.to_string();
    let tab = live.tab;
    finish_import(s, tab);
    keep_pictures(s, tab);
    if let Some(t) = s.tab_by_id_mut(tab) {
        t.doc.replicated_mut().set_stage_import_bytes(usize::MAX);
        live.session.client_mut().leave(now, t.doc.replicated_mut());
        // The goodbye and the close are outputs of the client; the pump hands them to the transport.
        live.session.pump(now, t.doc.replicated_mut());
    }
    unbind_tab(s, tab);
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

/// The tab stops being the copy of its room: the key is forgotten (so the saved workspace has no link any more), the
/// document stays as a local copy, and its pictures are embedded again.
fn unbind_tab(s: &mut Session, tab: TabId) {
    let Some(t) = s.tab_by_id_mut(tab) else { return };
    t.ext.set(RoomBinding(None));
    t.ext.set(SealedBinding(None));
    t.doc.replicated_mut().set_file_policy(FilePolicy::Embedded);
}

/// The relay deleted the room of `live` on request and closed the connection. The tab becomes a local copy the way
/// `collab.leave_room` makes one, minus the goodbye (the connection is gone): the document stays, the room's key and
/// the saved link are dropped, the chosen server stays, and the state remembers that the room was deleted.
pub(super) fn end_deleted_room(s: &mut Session, live: Live) {
    let tab = live.tab;
    finish_import(s, tab);
    keep_pictures(s, tab);
    if let Some(t) = s.tab_by_id_mut(tab) {
        t.doc.replicated_mut().set_stage_import_bytes(usize::MAX);
    }
    unbind_tab(s, tab);
    let url = live.session.client().url().clone();
    let token = live.session.client().auth_token().cloned();
    let room = live.room.to_string();
    s.ext.with::<CollabState, _>(|c| {
        c.server = Some(url);
        c.token = token;
        c.room_deleted = Some(room);
    });
}

/// Before a tab stops being a room's copy: takes every picture of the room that the blob cache has into the tab, so
/// the local copy keeps all the pictures this device holds. The document names the rest, and they show as empty
/// pictures.
pub(super) fn keep_pictures(s: &mut Session, tab: TabId) {
    let Some(t) = s.tab_by_id(tab) else { return };
    let missing: Vec<_> = t.doc.replicated().manifests().map(|(id, _)| id.clone()).filter(|id| t.doc.store().file(id).is_none()).collect();
    if !missing.is_empty() && s.load_cached_pictures(tab, &missing) > 0 {
        s.mark_dirty(crate::spec::Redraw::Scene);
    }
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
