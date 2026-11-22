//! Pictures in an encrypted room (docs/design/collab-e2e.md section 9).
//!
//! A room keeps a manifest `{mime, size}` per picture in the document, and the bytes in the tab's view, the local
//! blob cache and, sealed, on the relay. Two gates meet that model, and every command goes through them:
//!
//! * **In** ([`admit`]): a picture that is about to be stored must fit the room's limits. One over
//!   `MAX_IMAGE_BYTES` is `image_too_large`; pictures that would take the room over its count or its sealed bytes
//!   are `room_images_full`. `image.insert`, `clipboard.paste` and the library and `doc.open` merge all end here.
//! * **Out** ([`missing_pictures`], [`require_pictures`]): an export or a save needs the bytes of the pictures it
//!   shows. They come from the view, else the blob cache, else (online) from the relay.
//!
//! Outside an encrypted room none of this applies ([`Session::room_images`] is `None`).

use std::collections::BTreeSet;

use drafftink_protocol::SEAL_OVERHEAD;
use drafftink_scene::{Document, ElementKind, FileId, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cmd::collab::images::RoomImages;
use crate::refusal::Refusal;
use crate::session::Session;
use crate::state::TabId;

/// The refusal code of a picture over the room's size limit.
pub(crate) const IMAGE_TOO_LARGE: &str = "image_too_large";
/// The refusal code of pictures that do not fit the room's totals.
pub(crate) const ROOM_IMAGES_FULL: &str = "room_images_full";
/// The refusal code of an export that waits for pictures that are downloading.
pub(crate) const IMAGES_PENDING: &str = "images_pending";
/// The refusal code of an export that needs pictures this device cannot get.
pub(crate) const IMAGES_UNAVAILABLE: &str = "images_unavailable";

/// How long an export waits for pictures that are downloading before it gives up, in milliseconds.
const WAIT_MS: u64 = 30_000;
/// The longest suggestion argument text a refusal carries. A picture given as base64 is larger, and then the
/// refusal names the command and the argument without the rest.
const SUGGESTION_BYTES: usize = 256 * 1024;

/// A picture on its way into the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Incoming {
    /// The content hash.
    pub id: FileId,
    /// The size of the stored bytes.
    pub size: usize,
}

/// What a command does with a picture that is over the room's size limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Oversize {
    /// Store a smaller copy (`shrink: true`).
    Shrink,
    /// Refuse, and offer `shrink: true` (the command has that argument).
    Offer,
    /// Refuse, and say to save the picture smaller (paste, library, merge).
    Refuse,
}

/// The pictures that live image elements of `doc` use and `doc` holds the bytes of.
pub(crate) fn pictures_of(doc: &Document) -> Vec<Incoming> {
    let used: BTreeSet<&FileId> = doc
        .elements
        .iter()
        .filter(|e| !e.deleted)
        .filter_map(|e| match &e.kind {
            ElementKind::Image(i) => i.file_id.as_ref(),
            _ => None,
        })
        .collect();
    used.into_iter()
        .filter_map(|id| doc.files.get(id).map(|f| Incoming { id: id.clone(), size: f.bytes.len() }))
        .collect()
}

/// The size of a sealed picture on the relay.
fn sealed(size: usize) -> u64 {
    u64::try_from(size.saturating_add(SEAL_OVERHEAD)).unwrap_or(u64::MAX)
}

/// "6.2 MB" for a size, rounded up to a tenth, so a picture one byte over a limit never reads as the limit.
fn mb_up(bytes: u64) -> String {
    tenths((bytes.saturating_mul(10)).div_ceil(1 << 20))
}

/// "50 MB" for a limit, rounded down to a tenth.
fn mb_down(bytes: u64) -> String {
    tenths(bytes.saturating_mul(10) >> 20)
}

fn tenths(n: u64) -> String {
    match (n / 10, n % 10) {
        (whole, 0) => format!("{whole} MB"),
        (whole, tenth) => format!("{whole}.{tenth} MB"),
    }
}

fn too_large(cmd: &str, size: usize, max: usize, oversize: Oversize) -> Refusal {
    let (size_text, max_text) = (mb_up(size as u64), mb_down(max as u64));
    let way = match oversize {
        Oversize::Offer => "Run the command again with `shrink: true` to store a smaller copy, or save it smaller and insert it again.",
        Oversize::Shrink | Oversize::Refuse => "Save it smaller (for example as a JPEG) and try again.",
    };
    Refusal::custom(
        IMAGE_TOO_LARGE,
        cmd,
        format!("This picture is {size_text}. Pictures in a shared room can be at most {max_text}. {way}"),
    )
    .with_details(serde_json::json!({ "size": size, "max": max }))
}

/// The refusal of a picture that stays over the limit even as the smallest copy `shrink` can make.
pub(crate) fn too_large_even_shrunk(cmd: &str, size: usize, max: usize) -> Refusal {
    let (size_text, max_text) = (mb_up(size as u64), mb_down(max as u64));
    Refusal::custom(
        IMAGE_TOO_LARGE,
        cmd,
        format!(
            "This picture is {size_text}, and even its smallest copy is over the {max_text} that a picture in a shared room can take. Use a smaller picture."
        ),
    )
    .with_details(serde_json::json!({ "size": size, "max": max }))
}

/// Adds the offer to run `cmd` again with `shrink: true` to a refusal of [`IMAGE_TOO_LARGE`]. `args` are the
/// arguments of the call that was refused.
pub(crate) fn offer_shrink(refusal: Refusal, cmd: &str, args: Option<Value>) -> Refusal {
    if refusal.code != IMAGE_TOO_LARGE {
        return refusal;
    }
    let with_shrink = args
        .filter(|v| v.to_string().len() <= SUGGESTION_BYTES)
        .map(|mut v| {
            if let Value::Object(map) = &mut v {
                map.insert("shrink".to_owned(), Value::Bool(true));
            }
            v
        })
        .unwrap_or_else(|| serde_json::json!({ "shrink": true }));
    refusal.suggesting(cmd, Some(with_shrink))
}

/// Adds the offer to run `cmd` again with `missing_images: "placeholder"` to a refusal of [`IMAGES_UNAVAILABLE`].
pub(crate) fn offer_placeholder(refusal: Refusal, cmd: &str, args: Option<Value>) -> Refusal {
    if refusal.code != IMAGES_UNAVAILABLE {
        return refusal;
    }
    let mut args = args.unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(map) = &mut args {
        map.insert("missing_images".to_owned(), Value::String("placeholder".to_owned()));
    }
    refusal.suggesting(cmd, Some(args))
}

/// Checks pictures that are about to be stored against the limits of the room of the active tab. A picture the
/// document holds already costs nothing, and the same picture twice counts once. Nothing happens outside an
/// encrypted room. Call it before the first change.
pub(crate) fn admit(s: &Session, pics: &[Incoming], oversize: Oversize) -> Result<(), Refusal> {
    let Some(mut room) = s.room_images() else { return Ok(()) };
    let cmd = s.cmd();
    let held = |id: &FileId| s.tab().doc.replicated().manifest(id).is_some() || s.store().file(id).is_some();
    let mut seen: BTreeSet<&FileId> = BTreeSet::new();
    let fresh: Vec<&Incoming> = pics.iter().filter(|p| !held(&p.id) && seen.insert(&p.id)).collect();
    if let Some(big) = fresh.iter().find(|p| p.size > room.max_image_bytes) {
        return Err(too_large(cmd, big.size, room.max_image_bytes, oversize));
    }
    let (count, held_bytes) = (room.count, room.sealed_bytes);
    let mut adding = 0u64;
    for p in &fresh {
        if !room.has_room_for(p.size) {
            return Err(full(cmd, &room, count, held_bytes, adding.saturating_add(sealed(p.size))));
        }
        room.count = room.count.saturating_add(1);
        room.sealed_bytes = room.sealed_bytes.saturating_add(sealed(p.size));
        adding = adding.saturating_add(sealed(p.size));
    }
    Ok(())
}

fn full(cmd: &str, room: &RoomImages, count: u32, held: u64, adding: u64) -> Refusal {
    let text = if count >= room.max_room_images {
        format!(
            "This room already holds {count} pictures, which is the most a room takes. Remove some, or start a new room."
        )
    } else {
        format!(
            "This room already holds {} of pictures, and a room takes at most {}, so {} more do not fit. Remove some, or start a new room.",
            mb_up(held),
            mb_down(room.max_room_image_bytes),
            mb_up(adding)
        )
    };
    Refusal::custom(ROOM_IMAGES_FULL, cmd, text).with_details(serde_json::json!({
        "pictures": count,
        "bytes": held,
        "max_pictures": room.max_room_images,
        "max_bytes": room.max_room_image_bytes,
    }))
}

// ---- out: exports and saves ----

/// What an export does with a picture of a room that this device cannot get.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MissingImages {
    /// Refuse (`images_pending` while they download, else `images_unavailable`), so no export is silently
    /// incomplete.
    #[default]
    Refuse,
    /// Export without them: they show as empty frames, and the output says how many were left out.
    Placeholder,
}

/// An export that started waiting for pictures of a room, per tab.
#[derive(Debug, Default)]
struct Waiting {
    since: Option<(TabId, u64)>,
}

/// The pictures of `used` that the active room tab holds a manifest for, and whose bytes this device lacks even
/// after the blob cache was read. Empty outside a room.
pub(crate) fn missing_pictures(s: &mut Session, used: &BTreeSet<FileId>) -> Vec<FileId> {
    if s.room_images().is_none() {
        return Vec::new();
    }
    let lacking = |s: &Session| -> Vec<FileId> {
        used.iter()
            .filter(|id| s.tab().doc.replicated().manifest(id).is_some() && s.store().file(id).is_none())
            .cloned()
            .collect()
    };
    let first = lacking(s);
    if first.is_empty() {
        return first;
    }
    s.load_cached_room_images(&first);
    lacking(s)
}

/// The pictures that the elements `ids` show.
pub(crate) fn used_by<'a>(s: &Session, ids: impl IntoIterator<Item = &'a drafftink_scene::ElementId>) -> BTreeSet<FileId> {
    ids.into_iter()
        .filter_map(|id| s.element(id))
        .filter_map(|e| e.image_data().and_then(|d| d.file_id.clone()))
        .collect()
}

/// Makes the pictures `used` available for an export, or says why not. Returns how many were left out
/// (`Placeholder` only).
pub(crate) fn require_pictures(s: &mut Session, used: &BTreeSet<FileId>, policy: MissingImages) -> Result<u32, Refusal> {
    let missing = missing_pictures(s, used);
    let tab = s.tab().id;
    if missing.is_empty() {
        s.ext.with::<Waiting, _>(|w| w.since = None);
        return Ok(0);
    }
    let n = u32::try_from(missing.len()).unwrap_or(u32::MAX);
    if policy == MissingImages::Placeholder {
        s.ext.with::<Waiting, _>(|w| w.since = None);
        s.warn(format!(
            "{} of this room {} left out of the export and show as empty frames.",
            pictures(n),
            if n == 1 { "is" } else { "are" }
        ));
        return Ok(n);
    }
    let cmd = s.cmd();
    let online = s.room_images().is_some_and(|r| r.online);
    let now = s.ports.clock.mono_ms();
    let since = s.ext.with::<Waiting, _>(|w| match w.since {
        Some((t, at)) if t == tab => at,
        _ => {
            w.since = Some((tab, now));
            now
        }
    });
    if online && now.saturating_sub(since) < WAIT_MS {
        s.want_room_images(&missing);
        return Err(Refusal::custom(
            IMAGES_PENDING,
            cmd,
            format!(
                "{} of this room {} still downloading. Run the export again in a moment.",
                pictures(n),
                if n == 1 { "is" } else { "are" }
            ),
        )
        .with_details(serde_json::json!({ "pictures": n })));
    }
    s.ext.with::<Waiting, _>(|w| w.since = None);
    Err(Refusal::custom(
        IMAGES_UNAVAILABLE,
        cmd,
        format!(
            "{} of this room could not be loaded, so the export would have empty frames. Pass `missing_images: \"placeholder\"` to export without them.",
            pictures(n)
        ),
    )
    .with_details(serde_json::json!({ "pictures": n })))
}

/// "1 picture" or "3 pictures".
fn pictures(n: u32) -> String {
    if n == 1 { "1 picture".to_owned() } else { format!("{n} pictures") }
}

/// For a save: how many pictures of the live elements this device could not embed. They are looked up in the
/// blob cache first, and asked for when online, so a later save has them. Never refuses.
pub(crate) fn embeddable_pictures(s: &mut Session) -> u32 {
    if s.room_images().is_none() {
        return 0;
    }
    let ids: Vec<drafftink_scene::ElementId> = s.store().iter().map(|e| e.id.clone()).collect();
    let used = used_by(s, &ids);
    let missing = missing_pictures(s, &used);
    if missing.is_empty() {
        return 0;
    }
    if s.room_images().is_some_and(|r| r.online) {
        s.want_room_images(&missing);
    }
    u32::try_from(missing.len()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_the_limit_does_and_never_hide_one_byte_over() {
        let four = 4 * 1024 * 1024;
        assert_eq!(mb_down(four), "4 MB");
        assert_eq!(mb_up(four), "4 MB");
        assert_eq!(mb_up(four + 1), "4.1 MB");
        assert_eq!(mb_up(6_501_171), "6.2 MB");
        assert_eq!(mb_down(50 * 1024 * 1024), "50 MB");
        assert_eq!(mb_up(0), "0 MB");
    }

    #[test]
    fn counts_read_in_the_singular_and_the_plural() {
        assert_eq!(pictures(1), "1 picture");
        assert_eq!(pictures(3), "3 pictures");
    }
}
