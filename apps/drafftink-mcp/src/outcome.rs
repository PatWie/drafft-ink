//! What a tool call returns, independent of the MCP library: text and image blocks, and an error flag.
//!
//! A refusal is a tool error (`isError: true`) whose text is the refusal's plain words, so the model reads
//! how to proceed. A malformed call (an unknown tool) is a protocol error and is handled in `server`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_bridge::RpcError;
use drafftink_bridge::rpc::REFUSED;
use serde_json::Value;

/// One piece of a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Text.
    Text(String),
    /// A picture, as base64 with its media type.
    Image {
        /// Base64 data.
        data: String,
        /// For example `image/png`.
        mime: String,
    },
}

/// The result of a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// What to show the model.
    pub blocks: Vec<Block>,
    /// The call did not do what was asked.
    pub is_error: bool,
}

impl Outcome {
    /// A success with one text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self { blocks: vec![Block::Text(text.into())], is_error: false }
    }

    /// A failure with one text block.
    pub fn error(text: impl Into<String>) -> Self {
        Self { blocks: vec![Block::Text(text.into())], is_error: true }
    }

    /// All text, joined with newlines. For tests and logs.
    pub fn plain_text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.as_str()),
                Block::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A refusal or other bridge error as plain words, with the next step if the refusal names one.
pub fn error_text(e: &RpcError) -> String {
    let mut text = e.message.clone();
    if e.code == REFUSED
        && let Some(steps) = e.data.as_ref().and_then(|d| d.get("suggest")).and_then(Value::as_array)
    {
        for step in steps {
            let Some(command) = step.get("command").and_then(Value::as_str) else { continue };
            match step.get("args") {
                Some(args) => text.push_str(&format!("\nTry: `{command}` with {args}")),
                None => text.push_str(&format!("\nTry: `{command}`")),
            }
        }
    }
    text
}

/// Pulls `bytes: {base64, mime}` out of a result: a picture becomes an image block, text-like data becomes
/// text. Returns the blocks for it and removes the key, so a large base64 string is not shown twice.
fn take_bytes(result: &mut Value) -> Option<Block> {
    let obj = result.as_object_mut()?;
    let bytes = obj.get("bytes")?;
    let (data, mime) = (bytes.get("base64")?.as_str()?.to_owned(), bytes.get("mime")?.as_str()?.to_owned());
    obj.remove("bytes");
    if mime.starts_with("image/") && mime != "image/svg+xml" {
        return Some(Block::Image { data, mime });
    }
    let decoded = STANDARD.decode(data.as_bytes()).ok()?;
    Some(Block::Text(String::from_utf8_lossy(&decoded).into_owned()))
}

/// Shapes a successful command result.
pub fn success(mut result: Value) -> Outcome {
    let extra = take_bytes(&mut result);
    let mut blocks = vec![Block::Text(result.to_string())];
    blocks.extend(extra);
    Outcome { blocks, is_error: false }
}

/// Shapes the result of a batch. A batch that ran and failed is an error whose first words are the refusal.
pub fn batch(result: Value) -> Outcome {
    let Some(failed) = result.get("failed").filter(|f| !f.is_null()) else {
        return success(result);
    };
    let index = failed.get("index").and_then(Value::as_u64).unwrap_or(0);
    let id = failed.get("id").and_then(Value::as_str).unwrap_or("?");
    let why = failed.get("refusal").and_then(|r| r.get("message")).and_then(Value::as_str).unwrap_or("it refused");
    let tail = if result.get("rolled_back") == Some(&Value::Bool(true)) {
        "The whole batch was rolled back, so nothing changed."
    } else {
        "Earlier steps were kept."
    };
    Outcome {
        blocks: vec![
            Block::Text(format!("Step {index} (`{id}`) refused: {why} {tail}")),
            Block::Text(result.to_string()),
        ],
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_picture_becomes_an_image_block_without_repeating_the_base64() {
        let o = success(json!({ "bytes": { "base64": "AAAA", "mime": "image/png" }, "width": 2 }));
        assert_eq!(o.blocks.len(), 2);
        assert_eq!(o.blocks[0], Block::Text(r#"{"width":2}"#.to_owned()));
        assert_eq!(o.blocks[1], Block::Image { data: "AAAA".into(), mime: "image/png".into() });
    }

    #[test]
    fn svg_is_decoded_to_text() {
        let data = STANDARD.encode("<svg/>");
        let o = success(json!({ "bytes": { "base64": data, "mime": "image/svg+xml" } }));
        assert_eq!(o.blocks.last(), Some(&Block::Text("<svg/>".to_owned())));
    }

    #[test]
    fn a_failed_batch_starts_with_the_refusal_text() {
        let o = batch(
            json!({ "completed": 1, "rolled_back": true, "failed": { "index": 1, "id": "x.y", "refusal": { "message": "Do the other thing." } } }),
        );
        assert!(o.is_error);
        assert_eq!(
            o.plain_text().lines().next(),
            Some("Step 1 (`x.y`) refused: Do the other thing. The whole batch was rolled back, so nothing changed.")
        );
    }

    #[test]
    fn suggestions_are_appended_to_a_refusal() {
        let e = RpcError {
            code: REFUSED,
            message: "No.".into(),
            data: Some(json!({ "suggest": [ { "command": "view.center", "args": { "x": 1 } } ] })),
        };
        assert_eq!(error_text(&e), "No.\nTry: `view.center` with {\"x\":1}");
    }
}
