//! What a sealed client keeps about the pictures of its room: the ones it holds and must upload (offers), the ones it
//! must show (wants), the relay's list for this connection, and the requests in flight. Sans-IO: requests go into a
//! queue, answers come in through [`Images::on_response`](super::Images), time is an argument.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use drafftink_protocol::SEAL_OVERHEAD;
use drafftink_protocol::v2::{ImageName, RoomId, RoomLimits};
use drafftink_scene::FileId;
use drafftink_seal::RoomKeys;

use super::backoff::Retry;
use super::wire::{RoomTotals, raw_file_id};
use super::{ImageEvent, ImageProgress, ImageRefusal};
use crate::access::AccessDenial;
use crate::blobs::{BlobAuth, BlobOp, BlobRequest};

/// Most downloads in flight (a Free-plan relay counts each request, and a browser allows 6 per host).
pub const MAX_DOWNLOADS: usize = 3;
/// Most uploads in flight, a `HEAD` that checks a lost upload included.
pub const MAX_UPLOADS: usize = 2;
/// A request without an answer after this long is a failure of the network, in milliseconds. The transports give up
/// a little earlier, so this only catches a transport that lost a request.
pub const BLOB_TIMEOUT_MS: u64 = 120_000;

/// The step of an upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// Send the sealed picture.
    Put,
    /// Ask whether an upload whose answer was lost arrived.
    Head,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OfferPhase {
    /// The list of this connection is not known yet, so nobody knows whether the relay has the picture.
    Unlisted,
    /// The next request is due at `retry.not_before`.
    Due { step: Step, retry: Retry },
    /// A request is in flight; `attempt` failures so far.
    Sent { attempt: u32 },
    /// The relay has the picture. The bytes are let go.
    Stored,
    /// Not uploaded and never will be by this client: refused, or not a member. Counted in `failed`.
    Refused,
}

/// A picture this client holds and the relay should have.
#[derive(Debug)]
pub(super) struct Offer {
    pub(super) id: FileId,
    pub(super) name: ImageName,
    /// The bytes, until the relay has them.
    pub(super) bytes: Option<Arc<[u8]>>,
    pub(super) phase: OfferPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WantPhase {
    Due(Retry),
    Sent {
        attempt: u32,
    },
    /// Damaged or not allowed: not asked for again.
    Failed,
}

/// A picture the owner wants to show.
#[derive(Debug)]
pub(super) struct Want {
    pub(super) id: FileId,
    pub(super) name: ImageName,
    pub(super) phase: WantPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ListPhase {
    /// Not asked on this connection.
    Unasked,
    Due(Retry),
    Sent {
        attempt: u32,
    },
    Known,
    /// The relay does not take this client for a member.
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    List,
    Put(FileId),
    Head(FileId),
    Get(FileId),
}

#[derive(Debug)]
pub(super) struct Flight {
    pub(super) target: Target,
    pub(super) sent_at: u64,
}

/// What a call needs from the client besides the state of the pictures.
#[derive(Clone, Copy)]
pub(super) struct Env<'a> {
    pub(super) keys: &'a RoomKeys,
    pub(super) limits: &'a RoomLimits,
}

/// The pictures of one room. See the module docs.
#[derive(Debug)]
pub(in crate::sealed) struct Images {
    base: String,
    room: RoomId,
    auth: BlobAuth,
    pub(super) offers: Vec<Offer>,
    pub(super) wants: Vec<Want>,
    pub(super) list: ListPhase,
    /// The names the relay holds, as far as this connection knows.
    pub(super) listed: HashSet<ImageName>,
    pub(super) flights: HashMap<u64, Flight>,
    next_id: u64,
    out: VecDeque<BlobRequest>,
    pub(super) events: Vec<ImageEvent>,
    pub(super) totals: RoomTotals,
    /// The server's gate turned a request away, and the owner has not been told yet. Once per connection.
    denial: Option<AccessDenial>,
    denial_told: bool,
}

impl Images {
    /// Pictures of the room `room` at the server `base` (`http(s)://host[:port]`).
    pub(in crate::sealed) fn new(base: String, room: RoomId, auth: BlobAuth) -> Self {
        Images {
            base,
            room,
            auth,
            offers: Vec::new(),
            wants: Vec::new(),
            list: ListPhase::Unasked,
            listed: HashSet::new(),
            flights: HashMap::new(),
            next_id: 1,
            out: VecDeque::new(),
            events: Vec::new(),
            totals: RoomTotals::default(),
            denial: None,
            denial_told: false,
        }
    }

    /// Notes that the server's gate turned a request away. Only the first one of a connection is passed on.
    pub(super) fn note_denial(&mut self, denial: AccessDenial) {
        if !self.denial_told {
            self.denial_told = true;
            self.denial = Some(denial);
        }
    }

    /// The gate's refusal the owner has not been told yet.
    pub(in crate::sealed) fn take_denial(&mut self) -> Option<AccessDenial> {
        self.denial.take()
    }

    // ---- inputs from the owner ----------------------------------------------------------------------------------

    /// "I hold these bytes": upload them unless the relay has the name.
    pub(super) fn offer(&mut self, env: Env<'_>, id: &FileId, bytes: Arc<[u8]>) {
        let Some(raw) = raw_file_id(id) else { return };
        let name = env.keys.image_name(&raw);
        let phase = self.first_phase(&name);
        let needs_upload = matches!(phase, OfferPhase::Unlisted | OfferPhase::Due { .. });
        let bytes = needs_upload.then_some(bytes);
        match self.offers.iter_mut().find(|o| o.id == *id) {
            // Offering a picture again is how a refused one is tried again.
            Some(o) if o.phase == OfferPhase::Refused => {
                o.bytes = bytes;
                o.phase = phase;
            }
            Some(_) => {}
            None => self.offers.push(Offer { id: id.clone(), name, bytes, phase }),
        }
    }

    /// "Show me this picture": download it, again and again until it arrives.
    pub(super) fn want(&mut self, now: u64, env: Env<'_>, id: &FileId) {
        if let Some(bytes) = self.offers.iter().find(|o| o.id == *id).and_then(|o| o.bytes.clone()) {
            self.events.push(ImageEvent::Arrived { id: id.clone(), bytes });
            return;
        }
        if self.wants.iter().any(|w| w.id == *id) {
            return;
        }
        let Some(raw) = raw_file_id(id) else { return };
        let name = env.keys.image_name(&raw);
        self.wants.push(Want { id: id.clone(), name, phase: WantPhase::Due(Retry::at(now)) });
    }

    /// Where a new offer starts: stored, due, or waiting for the list.
    fn first_phase(&self, name: &ImageName) -> OfferPhase {
        match self.list {
            ListPhase::Known if self.listed.contains(name) => OfferPhase::Stored,
            ListPhase::Known => OfferPhase::Due { step: Step::Put, retry: Retry::at(0) },
            ListPhase::Denied => OfferPhase::Refused,
            _ => OfferPhase::Unlisted,
        }
    }

    // ---- connection ---------------------------------------------------------------------------------------------

    /// The connection is online: ask for the list.
    pub(in crate::sealed) fn on_online(&mut self, now: u64) {
        self.list = ListPhase::Due(Retry::at(now));
    }

    /// The connection is gone. In-flight work is forgotten (a late answer finds no request), offers and wants stay.
    pub(in crate::sealed) fn reset(&mut self) {
        self.flights.clear();
        self.out.clear();
        self.denial = None;
        self.denial_told = false;
        self.list = ListPhase::Unasked;
        self.listed.clear();
        for offer in &mut self.offers {
            if matches!(offer.phase, OfferPhase::Due { .. } | OfferPhase::Sent { .. }) {
                // The next list says whether the upload arrived.
                offer.phase = OfferPhase::Unlisted;
            }
        }
        for want in &mut self.wants {
            if matches!(want.phase, WantPhase::Due(_) | WantPhase::Sent { .. }) {
                want.phase = WantPhase::Due(Retry::at(0));
            }
        }
    }

    // ---- reading state ------------------------------------------------------------------------------------------

    pub(super) fn poll_request(&mut self) -> Option<BlobRequest> {
        self.out.pop_front()
    }

    pub(super) fn take_events(&mut self) -> Vec<ImageEvent> {
        std::mem::take(&mut self.events)
    }

    /// True while an upload is not finished: the owner shows the picture as not yet shared.
    pub(in crate::sealed) fn uploads_pending(&self) -> bool {
        self.offers
            .iter()
            .any(|o| matches!(o.phase, OfferPhase::Unlisted | OfferPhase::Due { .. } | OfferPhase::Sent { .. }))
    }

    pub(super) fn progress(&self) -> ImageProgress {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let uploading = self.uploads_in_flight();
        let downloading = self.downloads_in_flight();
        let waiting =
            self.offers.iter().filter(|o| matches!(o.phase, OfferPhase::Unlisted | OfferPhase::Due { .. })).count()
                + self.wants.iter().filter(|w| matches!(w.phase, WantPhase::Due(_))).count();
        let failed = self.offers.iter().filter(|o| o.phase == OfferPhase::Refused).count()
            + self.wants.iter().filter(|w| w.phase == WantPhase::Failed).count();
        ImageProgress {
            uploading: count(uploading),
            downloading: count(downloading),
            waiting: count(waiting),
            failed: count(failed),
            room_images: self.totals.images,
            room_image_bytes: self.totals.bytes,
        }
    }

    /// When to call [`drive`](Self::drive) next: the earliest retry that has a free place, the list, or the time a
    /// request is given up. A time in the past means now.
    pub(in crate::sealed) fn next_wake(&self) -> Option<u64> {
        if !self.out.is_empty() {
            return Some(0);
        }
        let mut wake: Option<u64> = None;
        let mut at = |t: u64| wake = Some(wake.map_or(t, |w| w.min(t)));
        if let ListPhase::Due(r) = self.list {
            at(r.not_before);
        }
        if self.uploads_in_flight() < MAX_UPLOADS {
            self.offers
                .iter()
                .filter_map(|o| match o.phase {
                    OfferPhase::Due { retry, .. } => Some(retry.not_before),
                    _ => None,
                })
                .for_each(&mut at);
        }
        if self.downloads_in_flight() < MAX_DOWNLOADS {
            self.wants
                .iter()
                .filter_map(|w| match w.phase {
                    WantPhase::Due(r) => Some(r.not_before),
                    _ => None,
                })
                .for_each(&mut at);
        }
        self.flights.values().for_each(|f| at(f.sent_at.saturating_add(BLOB_TIMEOUT_MS)));
        wake
    }

    fn uploads_in_flight(&self) -> usize {
        self.flights.values().filter(|f| matches!(f.target, Target::Put(_) | Target::Head(_))).count()
    }

    fn downloads_in_flight(&self) -> usize {
        self.flights.values().filter(|f| matches!(f.target, Target::Get(_))).count()
    }

    // ---- time ---------------------------------------------------------------------------------------------------

    /// Gives up requests that got no answer, refuses what is too large, and sends what is due. Call it while online.
    pub(super) fn drive(&mut self, now: u64, env: Env<'_>) {
        let expired: Vec<u64> = self
            .flights
            .iter()
            .filter(|(_, f)| now.saturating_sub(f.sent_at) >= BLOB_TIMEOUT_MS)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.on_response(now, env, crate::blobs::BlobResponse { id, result: Err("no answer in time".to_owned()) });
        }
        self.refuse_oversized(env.limits);
        self.start_list(now);
        self.start_uploads(now, env);
        self.start_downloads(now);
    }

    /// An offer above the relay's limit is refused here, without a request.
    fn refuse_oversized(&mut self, limits: &RoomLimits) {
        let max = usize::try_from(limits.max_image_bytes).unwrap_or(usize::MAX);
        for offer in &mut self.offers {
            let open = matches!(offer.phase, OfferPhase::Unlisted | OfferPhase::Due { .. });
            if open && offer.bytes.as_ref().is_some_and(|b| b.len() > max) {
                offer.phase = OfferPhase::Refused;
                offer.bytes = None;
                self.events.push(ImageEvent::Rejected { id: offer.id.clone(), why: ImageRefusal::TooLarge });
            }
        }
    }

    fn start_list(&mut self, now: u64) {
        let ListPhase::Due(retry) = self.list else { return };
        if retry.not_before <= now {
            let url = format!("{}/r/{}/f", self.base, self.room);
            self.send(now, Target::List, url, BlobOp::List);
            self.list = ListPhase::Sent { attempt: retry.attempt };
        }
    }

    fn start_uploads(&mut self, now: u64, env: Env<'_>) {
        while self.uploads_in_flight() < MAX_UPLOADS {
            let due = self
                .offers
                .iter()
                .position(|o| matches!(o.phase, OfferPhase::Due { retry, .. } if retry.not_before <= now));
            let Some(index) = due else { return };
            self.start_upload(now, env, index);
        }
    }

    fn start_upload(&mut self, now: u64, env: Env<'_>, index: usize) {
        let Some(offer) = self.offers.get_mut(index) else { return };
        let OfferPhase::Due { step, retry } = offer.phase else { return };
        let Some(bytes) = offer.bytes.clone() else {
            offer.phase = OfferPhase::Stored;
            return;
        };
        let (id, name) = (offer.id.clone(), offer.name);
        let (target, op) = match step {
            Step::Head => (Target::Head(id.clone()), BlobOp::Head),
            Step::Put => match env.keys.seal_image(&name, &bytes) {
                Ok(sealed) => (Target::Put(id.clone()), BlobOp::Put(sealed.into_bytes())),
                Err(e) => {
                    log::warn!("collab: a picture was not sealed: {e}");
                    offer.phase = OfferPhase::Refused;
                    offer.bytes = None;
                    return;
                }
            },
        };
        offer.phase = OfferPhase::Sent { attempt: retry.attempt };
        let url = format!("{}/r/{}/f/{name}", self.base, self.room);
        self.send(now, target, url, op);
    }

    fn start_downloads(&mut self, now: u64) {
        while self.downloads_in_flight() < MAX_DOWNLOADS {
            let due = self.wants.iter().position(|w| matches!(w.phase, WantPhase::Due(r) if r.not_before <= now));
            let Some(index) = due else { return };
            let Some(want) = self.wants.get_mut(index) else { return };
            let WantPhase::Due(retry) = want.phase else { return };
            want.phase = WantPhase::Sent { attempt: retry.attempt };
            let (id, name) = (want.id.clone(), want.name);
            let url = format!("{}/r/{}/f/{name}", self.base, self.room);
            self.send(now, Target::Get(id), url, BlobOp::Get);
        }
    }

    fn send(&mut self, now: u64, target: Target, url: String, op: BlobOp) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.flights.insert(id, Flight { target, sent_at: now });
        self.out.push_back(BlobRequest { id, url, op, auth: self.auth.clone() });
    }
}

/// The size a sealed picture of `plain_len` bytes has.
pub(super) fn sealed_len(plain_len: usize) -> u64 {
    u64::try_from(plain_len.saturating_add(SEAL_OVERHEAD)).unwrap_or(u64::MAX)
}
