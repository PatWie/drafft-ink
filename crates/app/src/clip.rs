//! Converts between the shell's clipboard content and the engine's clipboard payloads.
//!
//! The engine copies elements as a JSON value ([`ClipPayload::Elements`]). On the system clipboard that value
//! travels as the shell's private payload (so a paste into this app gets the elements back untouched) and as
//! text (so another program gets the JSON, which Excalidraw also reads when the compatibility setting is on).

use drafftink_engine::ports::{ClipKind, ClipPayload};
use drafftink_gpu::{ClipboardContent, ImageBytes};
use serde_json::Value;

/// What to put on the system clipboard for a payload.
pub(crate) fn to_content(payload: &ClipPayload) -> ClipboardContent {
    match payload {
        ClipPayload::Elements(v) => {
            let text = v.to_string();
            ClipboardContent { text: Some(text.clone()), images: Vec::new(), custom: Some(text.into_bytes()) }
        }
        ClipPayload::Text(t) | ClipPayload::Svg(t) => ClipboardContent::text(t.clone()),
        ClipPayload::Image { bytes, mime } => ClipboardContent {
            text: None,
            images: vec![ImageBytes { mime: mime.clone(), bytes: bytes.clone() }],
            custom: None,
        },
    }
}

/// What the system clipboard holds, as the payload of one kind. The engine asks kind by kind, best first.
pub(crate) fn payload_of(content: &ClipboardContent, kind: ClipKind) -> Option<ClipPayload> {
    match kind {
        ClipKind::Elements => content
            .custom
            .as_deref()
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .map(ClipPayload::Elements),
        ClipKind::Text => content.text.as_deref().filter(|t| !t.trim().is_empty()).map(|t| ClipPayload::Text(t.to_owned())),
        ClipKind::Svg => content
            .text
            .as_deref()
            .filter(|t| t.trim_start().starts_with("<svg"))
            .map(|t| ClipPayload::Svg(t.to_owned())),
        ClipKind::Image => content
            .images
            .first()
            .map(|i| ClipPayload::Image { bytes: i.bytes.clone(), mime: i.mime.clone() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn elements_round_trip_through_the_private_payload() {
        let payload = ClipPayload::Elements(json!({"type": "drafftink/clipboard", "elements": [1, 2]}));
        assert_eq!(payload_of(&to_content(&payload), ClipKind::Elements), Some(payload));
    }

    #[test]
    fn each_kind_is_read_on_its_own() {
        let both = ClipboardContent {
            text: Some("hello".into()),
            images: vec![ImageBytes { mime: "image/png".into(), bytes: vec![1, 2, 3] }],
            custom: None,
        };
        assert_eq!(payload_of(&both, ClipKind::Text), Some(ClipPayload::Text("hello".into())));
        assert_eq!(
            payload_of(&both, ClipKind::Image),
            Some(ClipPayload::Image { bytes: vec![1, 2, 3], mime: "image/png".into() })
        );
        assert_eq!(payload_of(&both, ClipKind::Elements), None);
        assert_eq!(payload_of(&both, ClipKind::Svg), None);
    }

    #[test]
    fn svg_text_is_also_svg_and_blank_text_is_nothing() {
        let svg = ClipboardContent::text("  <svg></svg>");
        assert!(matches!(payload_of(&svg, ClipKind::Svg), Some(ClipPayload::Svg(_))));
        assert_eq!(payload_of(&ClipboardContent::text("   "), ClipKind::Text), None);
        assert_eq!(payload_of(&ClipboardContent::default(), ClipKind::Image), None);
    }
}
