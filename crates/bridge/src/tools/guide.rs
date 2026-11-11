//! The one-page guide that teaches a model how to use drafft.ink. It is served as the MCP prompt
//! `drafftink_guide` and, in short form, in the `initialize` instructions.
//!
//! The command ids the text names are checked against the registry by a test. A command that the text
//! names but this build has not registered yet is listed at the end of the guide, so the model does not try
//! it blindly.

use drafftink_engine::Registry;

use super::Mode;

/// The name of the prompt.
pub const PROMPT_NAME: &str = "drafftink_guide";

const GUIDE: &str = r#"# Drawing in drafft.ink

drafft.ink is an infinite whiteboard. Everything on it is an *element*: rectangle, ellipse, diamond, line, arrow, freehand, text, image, frame. Every element has an id. You can also give an element a `name` and then refer to it as `@name` instead of its id. Coordinates are world units, x to the right, y down.

## How to work

1. **Look first.** Call `inspect` with `doc.outline`. It lists nodes with their labels, edges, groups, frames and bounds in a compact form. Use `element.query` to find things by meaning (text, kind, style, region, connection) and `element.at` to find what is at a point.
2. **Build diagrams from structure, not from coordinates.** For flowcharts, trees and graphs call the `diagram` tool with `diagram.create`: give `nodes` (a symbolic `id` and a `label`), `edges` (`from`, `to`, optional `label`) and a `layout`. It places the nodes, binds an arrow to each pair and returns the real ids. Use `diagram.extend` to add a connected node and `layout.apply` to re-lay-out existing elements.
3. **Single elements.** `element.create` makes a shape, line, arrow or text. Give important elements a `name`, so later calls can say `@name`.
4. **Change things by describing the change.** `style.set` for colours and strokes, `text.set` or `element.relabel` for text, `arrange.align` and `arrange.distribute` for order, `element.connect` to join two elements with a bound arrow. Prefer these over deleting and re-creating: ids, bindings and the user's own edits survive.
5. **Several edits that belong together go in one `batch`.** A batch is atomic: every step is checked first, one refusal rolls everything back, and the whole batch is one undo step. Name a step with `as` and use `"$name"` (its first created id) or `"$name.created"` (all of them) in later steps.
6. **Undo mistakes.** `history.undo` reverts the last step, a whole batch included. `recent` shows the newest commands by everyone, so you can see what the user changed since your last call.
7. **Check your work.** Read `doc.outline` again. With a renderer (attached or relayed mode) `view.screenshot` returns a picture of a region.

## Refusals

A command that cannot run says why in plain words and what to do instead, and it changes nothing. Read the text and follow it. `describe` shows a command's exact arguments, an example and its refusal codes. `commands` lists what exists and what can run right now.

## Tools

`inspect` runs read-only commands. One tool per family runs its editing commands (`element`, `style`, `diagram`, `arrange`, ...): call it with `command` and `args`. `batch`, `commands`, `describe`, `recent` and `status` work on the whole editor. `open`, `save` and `export` handle files.

## Examples

A flowchart in one call:

    diagram {"command": "diagram.create", "args": {
      "nodes": [{"id": "start", "label": "Start", "shape": "ellipse"},
                {"id": "check", "label": "Valid?", "shape": "diamond"},
                {"id": "ok", "label": "Save"}, {"id": "bad", "label": "Show error"}],
      "edges": [{"from": "start", "to": "check"}, {"from": "check", "to": "ok", "label": "yes"},
                {"from": "check", "to": "bad", "label": "no"}],
      "layout": {"algorithm": "layered", "direction": "down"}}}

Find and recolour:

    inspect {"command": "element.query", "args": {"filter": {"text": {"contains": "error"}}}}
    style {"command": "style.set", "args": {"ids": ["@error"], "props": {"stroke_color": "red-600"}}}

Two edits as one undo step:

    batch {"steps": [
      {"command": "element.create", "args": {"kind": "rectangle", "name": "box", "bounds": {"x": 0, "y": 0, "w": 160, "h": 80}}, "as": "box"},
      {"command": "element.relabel", "args": {"id": "$box", "text": "Hello"}}]}
"#;

/// The short text of the `initialize` result.
pub fn instructions(reg: &Registry, mode: Mode) -> String {
    let where_ = match mode {
        Mode::Headless => {
            "This server owns its own editor session without a window. Open and save files by path; export SVG (PNG needs the desktop app or the browser)."
        }
        Mode::Attached => "The user is watching the desktop app: your edits appear on their canvas as you make them.",
        Mode::Relay => {
            "You drive the user's browser tab. If `status` says it is not connected, ask the user to open the pairing link."
        }
        Mode::Desktop => "The user is watching the desktop app: your edits appear on their canvas as you make them.",
        Mode::Page => "You drive the browser tab that registered these tools: your edits appear on the user's canvas as you make them.",
    };
    let _ = reg;
    format!(
        "drafft.ink whiteboard. {where_}\n\
         Start with `inspect` and `doc.outline`, build diagrams with `diagram.create`, group related edits in `batch` (one undo step), and read refusals: they say what to do. \
         The prompt `{PROMPT_NAME}` has the full one-page guide."
    )
}

/// The parts of `text` between backticks or double quotes, where the guide names commands.
fn quoted(text: &str) -> impl Iterator<Item = &str> {
    text.split(['`', '"']).skip(1).step_by(2)
}

/// Command ids named in backticks or double quotes in `text`: `word.word`.
pub fn mentioned_commands(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in quoted(text) {
        for id in scan(span) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

fn scan(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<String>| {
        let w = std::mem::take(word);
        let w = w.trim_matches('.');
        if let Some((family, verb)) = w.split_once('.')
            && !family.is_empty()
            && !verb.is_empty()
            && !verb.contains('.')
            && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'_' || b == b'.')
            && !out.iter().any(|o| o == w)
        {
            out.push(w.to_owned());
        }
    };
    for c in text.chars() {
        if c.is_ascii_lowercase() || c == '_' || c == '.' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The full guide, with a closing note about commands this build lacks.
pub fn guide(reg: &Registry, mode: Mode) -> String {
    let missing: Vec<String> = mentioned_commands(GUIDE).into_iter().filter(|id| reg.find(id).is_none()).collect();
    let mut text = format!("{}\n\n{GUIDE}", instructions(reg, mode));
    if !missing.is_empty() {
        text.push_str(&format!(
            "\n## Not available in this build\n\nThese commands are named above but this build does not have them yet: {}. Use `commands` to see what exists.\n",
            missing.join(", ")
        ));
    }
    text
}

/// The guide text as written, for tests.
pub const fn guide_source() -> &'static str {
    GUIDE
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Ids of the catalog that no family has registered yet (`crates/engine/unregistered.txt`).
    fn unregistered() -> BTreeSet<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../engine/unregistered.txt");
        std::fs::read_to_string(path)
            .map(|t| t.lines().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect())
            .unwrap_or_default()
    }

    #[test]
    fn every_command_the_guide_names_exists_or_is_a_known_catalog_id() {
        let reg = Registry::global();
        let later = unregistered();
        for id in mentioned_commands(GUIDE) {
            assert!(reg.find(&id).is_some() || later.contains(&id), "the guide names `{id}`, which is not a command");
        }
    }

    #[test]
    fn the_guide_names_the_commands_it_promises() {
        let ids = mentioned_commands(GUIDE);
        for must in ["doc.outline", "diagram.create", "element.query", "history.undo", "style.set"] {
            assert!(ids.iter().any(|i| i == must), "{must}");
        }
    }

    #[test]
    fn a_command_this_build_lacks_is_listed_at_the_end() {
        let reg = Registry::global();
        let text = guide(&reg, Mode::Headless);
        let lacking: Vec<String> = mentioned_commands(GUIDE).into_iter().filter(|id| reg.find(id).is_none()).collect();
        assert_eq!(text.contains("Not available in this build"), !lacking.is_empty());
        for id in lacking {
            assert!(text.rsplit("Not available in this build").next().unwrap_or("").contains(&id));
        }
    }

    #[test]
    fn the_command_scanner_ignores_prose() {
        assert_eq!(mentioned_commands("Use `a.b`, then c.d on drafft.ink. Say \"e.f\"."), vec!["a.b", "e.f"]);
    }
}
