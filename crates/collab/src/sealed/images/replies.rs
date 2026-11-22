//! What the client does with the answer to a picture request: the retry rules of
//! `docs/design/collab-e2e.md` section 9.3.
//!
//! | Request | Answer | Result |
//! |---|---|---|
//! | `GET` | `200` | open, check the hash: `Arrived` or `Damaged` |
//! | `GET` | `404`, `429`, `503`, any other, network failure | try again after the backoff (`Retry-After` for `429`) |
//! | `GET` | `401` | final |
//! | `PUT` | `201`, `200` | stored |
//! | `PUT` | `413`, `507` | final: `Rejected` |
//! | `PUT` | `401`, `400` | final |
//! | `PUT` | `429`, `503`, any other | try again after the backoff |
//! | `PUT` | network failure | the answer may be lost: `HEAD` before the next `PUT` |
//! | `HEAD` | `200` with the length of this picture | stored |
//! | `HEAD` | `404`, or `200` with another length | `PUT` again (a name holding something else is overwritten) |
//! | list | `200` | offers the relay has are stored, the others are due |

use drafftink_scene::FileId;

use super::backoff::Retry;
use super::state::{Env, Images, ListPhase, OfferPhase, Step, Target, WantPhase, sealed_len};
use super::wire::{open_picture, parse_list, parse_totals};
use super::{ImageEvent, ImageRefusal};
use crate::blobs::{BlobReply, BlobResponse};

/// How a reply that is not a success is handled.
enum Failure {
    /// Not allowed, or a request that can never work: stop.
    Final,
    /// Try again later.
    Again { retry_after_ms: Option<u64> },
}

fn failure(status: u16, retry_after_ms: Option<u64>) -> Failure {
    match status {
        401 | 403 => Failure::Final,
        _ => Failure::Again { retry_after_ms },
    }
}

impl Images {
    /// Handles the answer to request `response.id`. An answer to a request that was forgotten (after a reconnect or a
    /// timeout) is ignored.
    pub(super) fn on_response(&mut self, now: u64, env: Env<'_>, response: BlobResponse) {
        let Some(flight) = self.flights.remove(&response.id) else { return };
        match flight.target {
            Target::List => self.on_list(now, response.result),
            Target::Put(id) => self.on_put(now, &id, response.result),
            Target::Head(id) => self.on_head(now, &id, response.result),
            Target::Get(id) => self.on_get(now, env, &id, response.result),
        }
    }

    fn on_list(&mut self, now: u64, result: Result<BlobReply, String>) {
        let ListPhase::Sent { attempt } = self.list else { return };
        let again =
            |this: &mut Self, retry_after_ms| this.list = ListPhase::Due(Retry::failed(attempt, now, retry_after_ms));
        match result {
            Ok(reply) if reply.status == 200 => match parse_list(&reply.body) {
                Some(entries) => {
                    self.totals.images = u32::try_from(entries.len()).unwrap_or(u32::MAX);
                    self.totals.bytes = entries.iter().map(|(_, size)| size).sum();
                    self.listed = entries.into_iter().map(|(name, _)| name).collect();
                    self.list = ListPhase::Known;
                    self.settle_offers(now);
                }
                None => again(self, None),
            },
            Ok(reply) => match failure(reply.status, reply.retry_after_ms) {
                Failure::Final => {
                    self.list = ListPhase::Denied;
                    self.refuse_waiting_offers();
                }
                Failure::Again { retry_after_ms } => again(self, retry_after_ms),
            },
            Err(_) => again(self, None),
        }
    }

    /// The list is known: what the relay has is stored, the rest is due.
    fn settle_offers(&mut self, now: u64) {
        for offer in &mut self.offers {
            if offer.phase != OfferPhase::Unlisted {
                continue;
            }
            if self.listed.contains(&offer.name) {
                offer.phase = OfferPhase::Stored;
                offer.bytes = None;
            } else {
                offer.phase = OfferPhase::Due { step: Step::Put, retry: Retry::at(now) };
            }
        }
    }

    fn refuse_waiting_offers(&mut self) {
        for offer in &mut self.offers {
            if matches!(offer.phase, OfferPhase::Unlisted) {
                offer.phase = OfferPhase::Refused;
                offer.bytes = None;
            }
        }
    }

    fn on_put(&mut self, now: u64, id: &FileId, result: Result<BlobReply, String>) {
        let Some(offer) = self.offers.iter_mut().find(|o| o.id == *id) else { return };
        let OfferPhase::Sent { attempt } = offer.phase else { return };
        offer.phase = match result {
            Ok(reply) if matches!(reply.status, 200 | 201) => {
                if let Some(totals) = parse_totals(&reply.body) {
                    self.totals = totals;
                }
                self.listed.insert(offer.name);
                offer.bytes = None;
                OfferPhase::Stored
            }
            Ok(reply) if matches!(reply.status, 413 | 507) => {
                let why = if reply.status == 413 { ImageRefusal::TooLarge } else { ImageRefusal::RoomFull };
                offer.bytes = None;
                self.events.push(ImageEvent::Rejected { id: offer.id.clone(), why });
                OfferPhase::Refused
            }
            Ok(reply) if reply.status == 400 => {
                log::warn!("collab: the relay refused a sealed picture as malformed");
                offer.bytes = None;
                OfferPhase::Refused
            }
            Ok(reply) => match failure(reply.status, reply.retry_after_ms) {
                Failure::Final => {
                    offer.bytes = None;
                    OfferPhase::Refused
                }
                Failure::Again { retry_after_ms } => {
                    OfferPhase::Due { step: Step::Put, retry: Retry::failed(attempt, now, retry_after_ms) }
                }
            },
            // The relay may have stored it and the answer got lost: look before sending it again.
            Err(_) => OfferPhase::Due { step: Step::Head, retry: Retry::failed(attempt, now, None) },
        };
    }

    fn on_head(&mut self, now: u64, id: &FileId, result: Result<BlobReply, String>) {
        let Some(offer) = self.offers.iter_mut().find(|o| o.id == *id) else { return };
        let OfferPhase::Sent { attempt } = offer.phase else { return };
        let expected = offer.bytes.as_ref().map(|b| sealed_len(b.len()));
        offer.phase = match result {
            Ok(reply) if reply.status == 200 && reply.content_length.is_none_or(|n| Some(n) == expected) => {
                self.listed.insert(offer.name);
                offer.bytes = None;
                OfferPhase::Stored
            }
            // Missing, or a name that holds something else: send the picture, which overwrites it.
            Ok(reply) if matches!(reply.status, 200 | 404) => {
                OfferPhase::Due { step: Step::Put, retry: Retry { attempt, not_before: now } }
            }
            Ok(reply) => match failure(reply.status, reply.retry_after_ms) {
                Failure::Final => {
                    offer.bytes = None;
                    OfferPhase::Refused
                }
                Failure::Again { retry_after_ms } => {
                    OfferPhase::Due { step: Step::Head, retry: Retry::failed(attempt, now, retry_after_ms) }
                }
            },
            Err(_) => OfferPhase::Due { step: Step::Head, retry: Retry::failed(attempt, now, None) },
        };
    }

    fn on_get(&mut self, now: u64, env: Env<'_>, id: &FileId, result: Result<BlobReply, String>) {
        let Some(index) = self.wants.iter().position(|w| w.id == *id) else { return };
        let Some(want) = self.wants.get_mut(index) else { return };
        let WantPhase::Sent { attempt } = want.phase else { return };
        match result {
            Ok(reply) if reply.status == 200 => match open_picture(env.keys, &want.name, &want.id, reply.body) {
                Some(bytes) => {
                    let id = want.id.clone();
                    self.wants.remove(index);
                    self.events.push(ImageEvent::Arrived { id, bytes });
                }
                None => {
                    want.phase = WantPhase::Failed;
                    self.events.push(ImageEvent::Damaged { id: want.id.clone() });
                }
            },
            Ok(reply) => {
                want.phase = match failure(reply.status, reply.retry_after_ms) {
                    Failure::Final => WantPhase::Failed,
                    Failure::Again { retry_after_ms } => WantPhase::Due(Retry::failed(attempt, now, retry_after_ms)),
                };
            }
            Err(_) => want.phase = WantPhase::Due(Retry::failed(attempt, now, None)),
        }
    }
}
