//! The pictures of an encrypted room, driven from [`Session::collab_poll`] (docs/design/collab-e2e.md section 9).
//!
//! The document names a picture by a manifest (`{mime, size}`); the bytes live in the tab's stores, in the local blob
//! cache (`Ns::Blobs`) and, sealed, on the relay. Once per change of what matters, [`reconcile`] brings the three
//! together:
//!
//! * a picture whose bytes the tab holds is **offered** to the client (which uploads it unless the relay has it) and
//!   written to the blob cache;
//! * a picture the tab lacks is read from the blob cache, else **wanted** from the relay when an element showing it
//!   is within a viewport of the visible area (the cost matters on the free plan, so a far picture is not fetched
//!   until the camera comes near it, or an export asks for it).
//!
//! "What matters" is a counter or a value that the poll compares: the manifest revision, the client turning online,
//! a picture arriving, the end of a staged import (the elements are all in then), and, only while some picture is
//! waiting for the camera, the camera. An idle session compares a few numbers and does nothing else. (An element
//! that a collaborator adds for a picture this device holds no bytes of, and has not fetched because it was far, is
//! noticed when the camera moves or the manifests change.)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use drafftink_collab::{ConnState, ImageEvent, ImageRefusal, Problem, ProblemAction};
use drafftink_geom::kurbo::Rect;
use drafftink_protocol::v2::RoomLimits;
use drafftink_scene::{ElementKind, FileId, SceneRead};

use super::images::megabytes;
use super::state::{CollabNotice, Live};
use crate::ports::Ns;
use crate::session::Session;
use crate::spec::Redraw;
use crate::state::{TabId, View};

/// What the last reconcile looked at.
#[derive(Debug, Default)]
struct Seen {
    manifests: Option<u64>,
    view: Option<View>,
}

/// How many of the pictures in the visible area are here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct VisibleCount {
    /// Distinct pictures shown by elements in the visible area.
    pub total: u32,
    /// Of those, the ones whose bytes are still missing and will come.
    pub missing: u32,
}

/// The picture state of one running room.
#[derive(Debug, Default)]
pub(super) struct Pictures {
    /// The room is encrypted, so its pictures are files. A plaintext room embeds them.
    sealed: bool,
    seen: Seen,
    /// Offered to the client since it last went online.
    offered: HashSet<FileId>,
    /// Asked of the client since it last went online.
    wanted: HashSet<FileId>,
    /// Damaged downloads: never asked for again.
    gave_up: HashSet<FileId>,
    /// Looked up in the blob cache already.
    cache_tried: HashSet<FileId>,
    /// The names in the blob cache, listed once.
    cached: Option<HashSet<String>>,
    /// A picture arrived (or something else asks for a look).
    due: bool,
    was_online: bool,
    /// A staged import was under way at the last poll.
    was_importing: bool,
    /// Some missing picture was not asked for (its element is far from the camera, or the client is offline).
    unasked: bool,
    visible: VisibleCount,
    /// How many times [`reconcile`] ran, for the test that an idle session does nothing.
    #[cfg(test)]
    pub(super) runs: u32,
}

impl Pictures {
    /// The state for a room that is (or is not) encrypted.
    pub(super) fn new(sealed: bool) -> Self {
        Self { sealed, ..Self::default() }
    }

    /// The pictures of the visible area, for the loading hint.
    pub(super) fn visible(&self) -> VisibleCount {
        self.visible
    }
}

/// One poll's look at the pictures: reconciles when something that matters changed. `None` when the tab is gone.
pub(super) fn step(s: &mut Session, live: &mut Live, now: u64) -> Option<()> {
    if !live.pictures.sealed {
        return Some(());
    }
    let online = live.session.client().status().state == ConnState::Online;
    let turned_online = online && !live.pictures.was_online;
    live.pictures.was_online = online;
    if turned_online {
        // The client forgot its offers on the way (and the relay may be new): offer and ask again.
        live.pictures.offered.clear();
        live.pictures.wanted.clear();
    }
    let tab = s.tab_by_id(live.tab)?;
    let (manifests, view) = (tab.doc.replicated().manifest_revision(), tab.view);
    let importing = tab.doc.replicated().import_progress().is_some();
    let p = &mut live.pictures;
    // The elements of a staged import appear slice by slice, so the pictures wait until the last one is in.
    let import_ended = std::mem::replace(&mut p.was_importing, importing) && !importing;
    let waiting_for_camera = p.unasked && p.seen.view != Some(view);
    if importing || (p.seen.manifests == Some(manifests) && !turned_online && !p.due && !waiting_for_camera && !import_ended) {
        return Some(());
    }
    p.seen = Seen { manifests: Some(manifests), view: Some(view) };
    p.due = false;
    reconcile(s, live, now, online)
}

/// A picture the tab holds.
struct Held {
    id: FileId,
    bytes: Arc<[u8]>,
}

fn reconcile(s: &mut Session, live: &mut Live, now: u64, online: bool) -> Option<()> {
    let _span = drafftink_profile::span("room_pictures");
    let tab_id = live.tab;
    #[cfg(test)]
    {
        live.pictures.runs += 1;
    }
    let ids: Vec<FileId> = s.tab_by_id(tab_id)?.doc.replicated().manifests().map(|(id, _)| id.clone()).collect();
    if live.pictures.cached.is_none() {
        let names = s.ports.storage.as_ref().and_then(|st| st.list(Ns::Blobs).ok()).unwrap_or_default();
        live.pictures.cached = Some(names.into_iter().map(|i| i.name).collect());
    }
    // 1. What the cache has and the tab lacks (a reload, a picture seen before) goes into the tab.
    let from_cache: Vec<FileId> = {
        let tab = s.tab_by_id(tab_id)?;
        let cached = live.pictures.cached.as_ref();
        let tried = &mut live.pictures.cache_tried;
        ids.iter()
            .filter(|id| tab.doc.store().file(id).is_none())
            .filter(|id| cached.is_some_and(|c| c.contains(id.as_str())))
            .filter(|id| tried.insert((*id).clone()))
            .cloned()
            .collect()
    };
    if !from_cache.is_empty() && s.load_cached_pictures(tab_id, &from_cache) > 0 {
        s.mark_dirty(Redraw::Scene);
    }
    // 2. What the tab holds goes to the relay and the cache.
    let tab = s.tab_by_id(tab_id)?;
    let mut held = Vec::new();
    let mut missing = HashSet::new();
    for id in &ids {
        match tab.doc.store().file(id) {
            Some(entry) => held.push(Held { id: id.clone(), bytes: Arc::clone(&entry.bytes) }),
            None => {
                missing.insert(id.clone());
            }
        }
    }
    let view = tab.view;
    for Held { id, bytes } in held {
        if online && live.pictures.offered.insert(id.clone()) {
            live.session.client_mut().offer_image(now, &id, Arc::clone(&bytes));
        }
        cache_put(s, &mut live.pictures, &id, &bytes);
    }
    // 3. What the tab lacks and is near the camera is asked of the relay.
    let area = look_around(s, tab_id, &missing, &view, &live.pictures.gave_up);
    let p = &mut live.pictures;
    p.visible = area.visible;
    let mut unasked = false;
    let waiting: Vec<&FileId> = missing.iter().filter(|id| !p.gave_up.contains(*id) && !p.wanted.contains(*id)).collect();
    for id in waiting {
        if online && area.near.contains(id) {
            p.wanted.insert(id.clone());
            live.session.client_mut().want_image(now, id);
        } else {
            unasked = true;
        }
    }
    p.unasked = unasked;
    Some(())
}

/// Writes `bytes` to the blob cache unless it has them. A failure leaves the picture to the next look.
fn cache_put(s: &mut Session, pictures: &mut Pictures, id: &FileId, bytes: &Arc<[u8]>) {
    let Some(cached) = pictures.cached.as_mut() else { return };
    if cached.contains(id.as_str()) {
        return;
    }
    let Some(storage) = s.ports.storage.as_mut() else { return };
    // A failed write leaves the name out, so the next look tries again.
    if storage.put_later(Ns::Blobs, id.as_str(), bytes.to_vec()).is_ok() {
        cached.insert(id.as_str().to_owned());
    }
}

/// Which missing pictures are near the camera, and what the visible area shows.
struct Area {
    /// Missing pictures with an element within a viewport of the visible rectangle.
    near: HashSet<FileId>,
    visible: VisibleCount,
}

fn look_around(s: &Session, tab: TabId, missing: &HashSet<FileId>, view: &View, gave_up: &HashSet<FileId>) -> Area {
    let mut area = Area { near: HashSet::new(), visible: VisibleCount::default() };
    if missing.is_empty() {
        return area;
    }
    let Some(tab) = s.tab_by_id(tab) else { return area };
    let (x, y, w, h) = view.visible();
    let shown = Rect::new(x, y, x + w, y + h);
    let sized = view.viewport.w > 0.0 && view.viewport.h > 0.0 && w.is_finite() && h.is_finite();
    // A viewport the shell never gave a size means everything is near.
    let around = if sized { shown.inflate(w, h) } else { Rect::new(f64::MIN, f64::MIN, f64::MAX, f64::MAX) };
    let store = tab.doc.store();
    let index = tab.doc.scene_index();
    let mut in_view: HashMap<&FileId, bool> = HashMap::new();
    for (id, bounds) in index.spatial().query_rect(&around) {
        let Some(element) = store.get(id).filter(|e| !e.deleted) else { continue };
        let ElementKind::Image(image) = &element.kind else { continue };
        let Some(file) = image.file_id.as_ref() else { continue };
        if tab.doc.replicated().manifest(file).is_none() {
            continue;
        }
        if missing.contains(file) {
            area.near.insert(file.clone());
        }
        if !sized || overlaps(&shown, &bounds) {
            in_view.insert(file, missing.contains(file) && !gave_up.contains(file));
        }
    }
    area.visible = VisibleCount {
        total: u32::try_from(in_view.len()).unwrap_or(u32::MAX),
        missing: u32::try_from(in_view.values().filter(|m| **m).count()).unwrap_or(u32::MAX),
    };
    area
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// Reacts to what the client says about a picture. `None` when the tab is gone.
pub(super) fn on_event(s: &mut Session, live: &mut Live, event: ImageEvent) -> Option<()> {
    let tab = live.tab;
    match event {
        ImageEvent::Arrived { id, bytes } => {
            s.tab_by_id(tab)?;
            // Not an edit: the document named the picture before, so the revision and the saved state stay.
            s.supply_picture(tab, &id, Arc::clone(&bytes));
            cache_put(s, &mut live.pictures, &id, &bytes);
            live.pictures.due = true;
            s.mark_dirty(Redraw::Scene);
        }
        ImageEvent::Damaged { id } => {
            live.pictures.gave_up.insert(id);
            live.pictures.due = true;
            s.collab_notify(CollabNotice::Info(
                "A picture in this room is damaged and cannot be shown. Its place stays empty.".to_owned(),
            ));
        }
        ImageEvent::Rejected { id, why } => {
            let size = s.tab_by_id(tab)?.doc.replicated().manifest(&id).map_or(0, |m| m.size);
            let limits = live.session.client().room_limits().copied().unwrap_or(RoomLimits::DEFAULT);
            let text = match why {
                ImageRefusal::TooLarge => format!(
                    "This picture is {}. Pictures in a shared room can be at most {}. Save it smaller (for example as a JPEG) and insert it again.",
                    megabytes(size),
                    megabytes(u64::from(limits.max_image_bytes))
                ),
                ImageRefusal::RoomFull => format!(
                    "This room already holds {} of pictures. Remove some, or start a new room.",
                    megabytes(limits.max_room_image_bytes)
                ),
            };
            s.collab_notify(CollabNotice::Problem(Problem { code: None, text, action: ProblemAction::ReduceSize, fatal: false }));
        }
    }
    Some(())
}
