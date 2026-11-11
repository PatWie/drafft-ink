//! The arguments of `doc.drop` for files dropped on the window.
//!
//! The engine decides what each file is by its content, so the shell sends the name, the bytes and the media
//! type the system gave, and nothing else. Both shells (the desktop window and the browser) deliver
//! [`DroppedFile`]s, so both build the command's arguments here.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_geom::kurbo::Point;
use drafftink_gpu::DroppedFile;
use serde_json::{Value, json};

/// The arguments of `doc.drop` for `files`. Images and merged elements go to the pointer when it is over the
/// canvas, else to the viewport centre (no `at`). A point is `[x, y]` in command arguments; an object is
/// refused as `bad_args`.
pub(crate) fn drop_args(files: &[DroppedFile], pointer: Option<Point>) -> Value {
    let items: Vec<Value> = files
        .iter()
        .map(|f| {
            let mime = (!f.mime.is_empty()).then_some(f.mime.as_str());
            json!({
                "name": f.name,
                "bytes": { "base64": STANDARD.encode(&f.bytes), "name": f.name, "mime": mime },
                "mime": mime,
            })
        })
        .collect();
    match pointer {
        Some(p) => json!({ "items": items, "at": [p.x, p.y] }),
        None => json!({ "items": items }),
    }
}
