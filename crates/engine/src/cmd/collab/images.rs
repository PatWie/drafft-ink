//! The pictures of an encrypted room, as the commands see them (docs/design/collab-e2e.md section 9).
//!
//! In a room the document holds a manifest `{mime, size}` per picture, and the bytes live in the tab's view and in
//! the local blob cache. A command that needs the bytes (an export, a save) asks [`Session::room_images`] which
//! pictures are missing and [`Session::want_room_images`] to fetch them. Commands never touch a socket: the client
//! downloads while [`Session::collab_poll`] runs.

use drafftink_collab::ConnState;
use drafftink_protocol::{MAX_IMAGE_BYTES, MAX_ROOM_IMAGE_BYTES, MAX_ROOM_IMAGES, SEAL_OVERHEAD};
use drafftink_scene::{FileEntry, FileId, SceneRead};

use crate::ports::Ns;

use super::state::CollabState;
use crate::cmd::tab::SealedBinding;
use crate::session::Session;
use crate::state::TabId;

/// A size for a message, in the unit of the limits: `4 MB`, `6.2 MB` (mebibytes, written as people write them).
pub(crate) fn megabytes(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;
    if bytes.is_multiple_of(MIB) {
        format!("{} MB", bytes / MIB)
    } else {
        // At least 0.1, so that a small picture does not read as 0.
        format!("{:.1} MB", (bytes as f64 / MIB as f64).max(0.1))
    }
}

/// What an encrypted room tab holds and allows. The commands that need pictures (`image.insert`, `doc.export`,
/// `doc.save`) read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoomImages {
    /// Pictures in the document (manifests).
    pub count: u32,
    /// Their sealed size on the relay: the sum of `size + SEAL_OVERHEAD`.
    pub sealed_bytes: u64,
    /// The pictures whose bytes this device does not hold yet.
    pub missing: Vec<FileId>,
    /// The client is connected and joined, so a download can succeed now.
    pub online: bool,
    /// Largest picture before sealing.
    pub max_image_bytes: usize,
    /// Most sealed bytes in the room.
    pub max_room_image_bytes: u64,
    /// Most pictures in the room.
    pub max_room_images: u32,
}

impl RoomImages {
    /// True when one more picture of `size` bytes still fits the room's totals.
    pub(crate) fn has_room_for(&self, size: usize) -> bool {
        let sealed = u64::try_from(size.saturating_add(SEAL_OVERHEAD)).unwrap_or(u64::MAX);
        self.count < self.max_room_images && self.sealed_bytes.saturating_add(sealed) <= self.max_room_image_bytes
    }
}

impl Session {
    /// The pictures of the active tab when it is an encrypted room, `None` otherwise.
    pub(crate) fn room_images(&self) -> Option<RoomImages> {
        let tab = self.tab();
        tab.ext.get::<SealedBinding>().filter(|b| b.0.is_some())?;
        let replicated = tab.doc.replicated();
        let (mut count, mut sealed_bytes, mut missing) = (0u32, 0u64, Vec::new());
        for (id, manifest) in replicated.manifests() {
            count = count.saturating_add(1);
            sealed_bytes = sealed_bytes
                .saturating_add(manifest.size)
                .saturating_add(u64::try_from(SEAL_OVERHEAD).unwrap_or(u64::MAX));
            if self.store().file(id).is_none() {
                missing.push(id.clone());
            }
        }
        let live = self.ext.get::<CollabState>().and_then(|c| c.live.as_ref()).filter(|live| live.tab == tab.id);
        let online = live.is_some_and(|live| live.session.client().status().state == ConnState::Online);
        // The relay's own limits once it announced them, else the ceilings every relay keeps.
        let limits = live.and_then(|live| live.session.client().room_limits());
        Some(RoomImages {
            count,
            sealed_bytes,
            missing,
            online,
            max_image_bytes: limits.map_or(MAX_IMAGE_BYTES, |l| usize::try_from(l.max_image_bytes).unwrap_or(usize::MAX)),
            max_room_image_bytes: limits.map_or(MAX_ROOM_IMAGE_BYTES, |l| l.max_room_image_bytes),
            max_room_images: limits.map_or(MAX_ROOM_IMAGES, |l| l.max_room_images),
        })
    }

    /// Asks the room client to download `ids` now, whatever the viewport says (an export needs them). Does nothing
    /// when the active tab has no running client. The bytes arrive during [`Session::collab_poll`].
    pub(crate) fn want_room_images(&mut self, ids: &[FileId]) {
        let now = self.ports.clock.mono_ms();
        let tab = self.tab().id;
        self.ext.with::<CollabState, _>(|c| {
            if let Some(live) = c.live.as_mut().filter(|l| l.tab == tab) {
                for id in ids {
                    live.session.client_mut().want_image(now, id);
                }
            }
        });
    }
}

impl Session {
    /// Reads the pictures `ids` from the local blob cache into the active tab. Returns how many were taken. The
    /// cache holds what this device downloaded or inserted, so an export works offline for every picture it has
    /// seen.
    pub(crate) fn load_cached_room_images(&mut self, ids: &[FileId]) -> usize {
        let tab = self.tab().id;
        self.load_cached_pictures(tab, ids)
    }

    /// Like [`Session::load_cached_room_images`] for the tab `tab`.
    pub(super) fn load_cached_pictures(&mut self, tab: TabId, ids: &[FileId]) -> usize {
        let mut loaded = 0;
        for id in ids {
            let bytes = self.ports.storage.as_ref().and_then(|st| st.get(Ns::Blobs, id.as_str()).ok().flatten());
            let Some(bytes) = bytes else { continue };
            if self.supply_picture(tab, id, bytes.into()) {
                loaded += 1;
            }
        }
        loaded
    }

    /// Puts the bytes of the picture `id` into `tab`, if the document names that picture and does not have its bytes
    /// yet. The bytes must hash to `id`. It is no edit: no undo step, no change of the saved state.
    pub(super) fn supply_picture(&mut self, tab: TabId, id: &FileId, bytes: std::sync::Arc<[u8]>) -> bool {
        let Some(t) = self.tab_by_id_mut(tab) else { return false };
        let Some(mime) = t.doc.replicated().manifest(id).map(|m| m.mime.clone()) else { return false };
        t.doc.supply_file(FileEntry { mime, bytes }).unwrap_or(false)
    }
}
