//! What a tab with pictures must pass before it joins an encrypted room (docs/design/collab-e2e.md section 9.6).
//!
//! A local document embeds its pictures. In a room they are sealed files, so the room's limits apply to them, and the
//! check runs before anything is sent or changed: a refusal leaves the tab as it was.

use drafftink_protocol::{MAX_IMAGE_BYTES, MAX_ROOM_IMAGE_BYTES, MAX_ROOM_IMAGES, SEAL_OVERHEAD};
use drafftink_scene::{ElementKind, FileId, SceneRead};

use super::images::megabytes;
use crate::refusal::Refusal;
use crate::session::Session;

/// A picture of the tab, as the message names it.
struct Picture {
    /// The element that shows it, or `None` for a picture no element shows.
    element: Option<String>,
    size: u64,
}

impl Picture {
    fn name(&self) -> String {
        match &self.element {
            Some(element) => format!("the picture of element `{element}`"),
            None => "a picture that no element shows".to_owned(),
        }
    }
}

/// The pictures of the active tab, in z order, each once. Pictures no element shows come last.
fn pictures(s: &Session) -> Vec<Picture> {
    let store = s.store();
    let mut seen: Vec<FileId> = Vec::new();
    let mut out = Vec::new();
    for element in store.iter() {
        let ElementKind::Image(image) = &element.kind else { continue };
        let Some(id) = image.file_id.as_ref().filter(|id| !seen.contains(id)) else { continue };
        let Some(file) = store.file(id) else { continue };
        seen.push(id.clone());
        out.push(Picture { element: Some(element.id.to_string()), size: file.bytes.len() as u64 });
    }
    for file in store.files().filter(|(id, _)| !seen.contains(id)).map(|(_, file)| file) {
        out.push(Picture { element: None, size: file.bytes.len() as u64 });
    }
    out
}

/// Refuses with `local_too_large_for_room` when the active tab's pictures do not fit an encrypted room: more than
/// [`MAX_ROOM_IMAGES`], one over [`MAX_IMAGE_BYTES`], or more than [`MAX_ROOM_IMAGE_BYTES`] sealed in total. The
/// message names the first picture that breaks a rule.
pub(super) fn check_local_pictures(s: &Session) -> Result<(), Refusal> {
    let all = pictures(s);
    if all.is_empty() {
        return Ok(());
    }
    let refuse = |text: String| {
        Refusal::custom(
            "local_too_large_for_room",
            s.cmd(),
            format!("{text} Nothing was sent and this tab is unchanged. Make the pictures smaller or remove some, then join again. To keep this tab as it is, open a new tab with `tab.new` and join from there."),
        )
        .suggesting("tab.new", None)
    };
    if let Some(big) = all.iter().find(|p| p.size > MAX_IMAGE_BYTES as u64) {
        return Err(refuse(format!(
            "{} is {}, and pictures in a shared room can be at most {}.",
            capitalized(&big.name()),
            megabytes(big.size),
            megabytes(MAX_IMAGE_BYTES as u64)
        )));
    }
    if let Some(over) = all.get(MAX_ROOM_IMAGES as usize) {
        return Err(refuse(format!(
            "This tab has {} pictures, and a shared room holds at most {MAX_ROOM_IMAGES}. {} is the first one over the limit.",
            all.len(),
            capitalized(&over.name())
        )));
    }
    let mut total = 0u64;
    for p in &all {
        total = total.saturating_add(p.size).saturating_add(SEAL_OVERHEAD as u64);
        if total > MAX_ROOM_IMAGE_BYTES {
            return Err(refuse(format!(
                "The pictures of this tab add up to more than {}, which is all a shared room holds. {} is the first one that does not fit.",
                megabytes(MAX_ROOM_IMAGE_BYTES),
                capitalized(&p.name())
            )));
        }
    }
    Ok(())
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}
