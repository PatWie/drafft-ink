//! The headline stories, driven through the agent bridge of the native app: the same commands the browser suite
//! sends through `window.drafftink`, so a story that passes in both shells shows that the shells carry the same app.
//! `e2e/stories.txt` lists them; `fn story_<id>` is the test of the story `<id>`.

mod common;

use common::{count, list, make_box, num, row, text, undo};
use drafftink_e2e::{E2eError, hex, start_or_skip};
use serde_json::{Value, json};

type Test = Result<(), E2eError>;

fn bindings(r: &Value) -> (Option<String>, Option<String>) {
    let end = |k: &str| r.pointer(&format!("/bindings/{k}")).and_then(Value::as_str).map(str::to_owned);
    (end("start"), end("end"))
}

#[test]
fn story_arrow_attached() -> Test {
    let Some(mut app) = start_or_skip("story_arrow_attached")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    let rect = make_box(&mut app, "rectangle", 0.0, 0.0, 100.0, 60.0, None)?;
    let oval = make_box(&mut app, "ellipse", 400.0, 0.0, 100.0, 60.0, None)?;
    let arrow = text(&app.exec("element.connect", json!({ "from": rect, "to": oval }))?, "/id")?.to_owned();
    let base = row(&mut app, &arrow)?;
    assert_eq!(
        bindings(&base),
        (Some(rect.clone()), Some(oval.clone())),
        "the arrow starts on the rectangle and ends on the ellipse"
    );
    let (bx, by, bw, bh) =
        (num(&base, "/bounds/x")?, num(&base, "/bounds/y")?, num(&base, "/bounds/w")?, num(&base, "/bounds/h")?);
    assert!(bw > 250.0, "the arrow spans the gap, w = {bw}");

    // Each change: the arrow follows and stays bound; one undo restores the exact old geometry and bindings.
    let changes: [(&str, &str, Value); 6] = [
        ("move the rectangle", "element.move", json!({ "ids": [rect], "dx": -50, "dy": 0, "snap": "none" })),
        ("move the ellipse", "element.move", json!({ "ids": [oval], "dx": 80, "dy": 0, "snap": "none" })),
        (
            "resize the rectangle",
            "element.resize",
            json!({ "ids": [rect], "handle": "e", "dx": 60, "dy": 0, "snap": "none" }),
        ),
        ("rotate the ellipse", "element.rotate", json!({ "ids": [oval], "angle": 90 })),
        ("rotate both", "element.rotate", json!({ "ids": [rect, oval], "angle": 90 })),
        ("move both", "element.move", json!({ "ids": [rect, oval], "dx": 100, "dy": 100, "snap": "none" })),
    ];
    for (what, id, args) in changes {
        app.exec(id, args)?;
        let after = row(&mut app, &arrow)?;
        assert_eq!(bindings(&after), (Some(rect.clone()), Some(oval.clone())), "{what}: still bound at both ends");
        undo(&mut app, 1)?;
        let back = row(&mut app, &arrow)?;
        for (key, want) in [("x", bx), ("y", by), ("w", bw), ("h", bh)] {
            let got = num(&back, &format!("/bounds/{key}"))?;
            assert!((got - want).abs() < 0.01, "{what}: undo restores {key}: {got} vs {want}");
        }
        assert_eq!(bindings(&back), (Some(rect.clone()), Some(oval.clone())), "{what}: undo keeps the bindings");
    }
    // Specific geometry of two of them.
    app.exec("element.move", json!({ "ids": [rect], "dx": -50, "dy": 0, "snap": "none" }))?;
    let moved = row(&mut app, &arrow)?;
    assert!(
        (num(&moved, "/bounds/x")? - (bx - 50.0)).abs() < 0.5 && (num(&moved, "/bounds/w")? - (bw + 50.0)).abs() < 0.5
    );
    undo(&mut app, 1)?;
    app.exec("element.rotate", json!({ "ids": [oval], "angle": 90 }))?;
    let turned = row(&mut app, &arrow)?;
    assert!(
        num(&turned, "/bounds/y")? < 0.0 && num(&turned, "/bounds/h")? > 20.0,
        "the end went up to the top of the turned ellipse"
    );
    undo(&mut app, 1)?;

    // Moving the arrow alone frees both ends.
    app.exec("element.move", json!({ "ids": [arrow], "dx": 0, "dy": 100, "snap": "none" }))?;
    assert_eq!(bindings(&row(&mut app, &arrow)?), (None, None));
    undo(&mut app, 1)?;

    // Deleting the ellipse frees the end; undo binds it again under the same id.
    app.exec("element.delete", json!({ "ids": [oval] }))?;
    assert_eq!(bindings(&row(&mut app, &arrow)?), (Some(rect.clone()), None));
    undo(&mut app, 1)?;
    assert_eq!(bindings(&row(&mut app, &arrow)?), (Some(rect), Some(oval.clone())));
    assert_eq!(text(&row(&mut app, &oval)?, "/kind")?, "ellipse");
    let ring = app.recent(5)?;
    assert!(
        ring.iter().all(|e| e.get("source").and_then(Value::as_str) == Some("agent")),
        "bridge calls have origin agent: {ring:?}"
    );
    Ok(())
}

#[test]
fn story_grouping() -> Test {
    let Some(mut app) = start_or_skip("story_grouping")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    let a = make_box(&mut app, "rectangle", 0.0, 0.0, 100.0, 60.0, Some("#ffcc00"))?;
    let _b = make_box(&mut app, "ellipse", 200.0, 0.0, 100.0, 60.0, Some("#ffcc00"))?;
    let c = make_box(&mut app, "diamond", 400.0, 0.0, 100.0, 60.0, Some("#ffcc00"))?;
    let _d = make_box(&mut app, "rectangle", 600.0, 0.0, 100.0, 60.0, Some("#ffcc00"))?;

    let few = app.refusal("group.create", json!({ "ids": [c] }))?;
    assert_eq!(few.code, "too_few");
    assert!(few.message.contains("at least 2 elements"), "{}", few.message);
    let made = app.exec("group.create", json!({ "ids": [a, c] }))?;
    let group = text(&made, "/group")?.to_owned();
    let again = app.refusal("group.create", json!({ "ids": [a, c] }))?;
    assert_eq!(again.code, "already_grouped");
    assert!(again.message.contains("already form one group at this level"));

    app.exec("selection.clear", json!({}))?;
    let clicked = app.exec("selection.select_at", json!({ "point": [50, 30] }))?;
    assert_eq!(clicked.pointer("/selected"), Some(&json!([a, c])), "a click selects the whole group");
    let entered = app.exec("group.enter", json!({ "select": c }))?;
    assert_eq!(text(&entered, "/editing_group")?, group);
    app.exec("element.move", json!({ "dx": 0, "dy": 100 }))?;
    assert_eq!(num(&row(&mut app, &a)?, "/bounds/y")?, 0.0, "the other member stayed");
    assert_eq!(num(&row(&mut app, &c)?, "/bounds/y")?, 100.0, "the entered member moved");
    let left = app.exec("group.exit", json!({}))?;
    assert!(left.get("editing_group").is_some_and(Value::is_null));
    let not_in = app.refusal("group.exit", json!({}))?;
    assert_eq!(not_in.code, "not_in_group");
    assert!(not_in.message.contains("not inside a group"));

    let gone = app.exec("group.ungroup", json!({}))?;
    assert_eq!(gone.pointer("/removed"), Some(&json!([group])));
    let none = app.refusal("group.ungroup", json!({}))?;
    assert_eq!(none.code, "no_group");
    undo(&mut app, 1)?;
    assert_eq!(num(&app.exec("doc.inspect", json!({}))?, "/groups")?, 1.0, "undo of the ungroup");
    undo(&mut app, 2)?;
    assert_eq!(num(&row(&mut app, &c)?, "/bounds/y")?, 0.0, "undo of the member move");
    Ok(())
}

#[test]
fn story_flowchart() -> Test {
    let Some(mut app) = start_or_skip("story_flowchart")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    let made = app.exec(
        "diagram.create",
        json!({
            "nodes": [
                { "id": "start", "label": "Start", "shape": "ellipse", "name": "start" },
                { "id": "check", "label": "Valid?", "shape": "diamond", "name": "check" },
                { "id": "ok", "label": "Process", "name": "ok" },
                { "id": "bad", "label": "Reject", "name": "bad" },
            ],
            "edges": [
                { "from": "start", "to": "check" },
                { "from": "check", "to": "ok", "label": "yes" },
                { "from": "check", "to": "bad", "label": "no" },
            ],
            "layout": {},
        }),
    )?;
    let ids = made.get("nodes").and_then(Value::as_object).ok_or_else(|| E2eError::Shape("no nodes".into()))?;
    assert_eq!(ids.len(), 4);
    let outline = app.exec("doc.outline", json!({ "format": "json" }))?;
    assert_eq!(list(&outline, "/outline/nodes")?.len(), 4);
    assert_eq!(list(&outline, "/outline/edges")?.len(), 3);
    let check_id = ids.get("check").and_then(Value::as_str).unwrap_or_default();
    let check = list(&outline, "/outline/nodes")?
        .iter()
        .find(|n| n.get("id").and_then(Value::as_str) == Some(check_id))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(text(&check, "/kind")?, "diamond");
    assert_eq!(text(&check, "/label")?, "Valid?");
    let text_outline = app.exec("doc.outline", json!({ "format": "text" }))?;
    assert!(text(&text_outline, "/text")?.contains("\"Valid?\" diamond"));

    let ys = app.exec(
        "element.query",
        json!({ "filter": { "ids": ["@start", "@check", "@ok"] }, "fields": ["name", "bounds"] }),
    )?;
    let y = |name: &str| -> f64 {
        list(&ys, "/items")
            .ok()
            .and_then(|items| {
                items
                    .iter()
                    .find(|i| i.get("name").and_then(Value::as_str) == Some(name))
                    .and_then(|i| i.pointer("/bounds/y").and_then(Value::as_f64))
            })
            .unwrap_or(f64::NAN)
    };
    assert!(y("start") < y("check") && y("check") < y("ok"), "the layers run from the top");
    assert_eq!(num(&app.exec("history.state", json!({}))?, "/undo_depth")?, 1.0, "the whole diagram is one undo step");

    app.exec("element.relabel", json!({ "id": "@bad", "text": "Reject request" }))?;
    app.exec(
        "element.create",
        json!({ "kind": "rectangle", "bounds": { "x": 120, "y": 480, "w": 120, "h": 60 }, "name": "log" }),
    )?;
    app.exec("element.relabel", json!({ "id": "@log", "text": "Write log" }))?;
    let linked = app.exec("element.connect", json!({ "from": "@bad", "to": "@log", "label": "then" }))?;
    assert_eq!(list(&linked, "/created")?.len(), 2);
    app.exec("diagram.extend", json!({ "from": "@ok", "direction": "down", "label": "Done" }))?;
    let grown = app.exec("doc.outline", json!({ "format": "json" }))?;
    assert_eq!((list(&grown, "/outline/nodes")?.len(), list(&grown, "/outline/edges")?.len()), (6, 5));
    assert_eq!(app.refusal("diagram.extend", json!({ "from": "@start", "direction": "sideways" }))?.code, "bad_args");

    undo(&mut app, 5)?;
    let back = app.exec("doc.outline", json!({ "format": "json" }))?;
    assert_eq!(list(&back, "/outline/nodes")?.len(), 4);
    let bad = list(&back, "/outline/nodes")?
        .iter()
        .find(|n| n.get("name").and_then(Value::as_str) == Some("bad"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(text(&bad, "/label")?, "Reject", "with the old label");
    let dup = app.refusal(
        "diagram.create",
        json!({ "nodes": [{ "id": "a" }], "edges": [{ "from": "a", "to": "zz" }], "layout": {} }),
    )?;
    assert_eq!(dup.code, "unknown_node");
    assert!(
        dup.message.contains("Edge 0 refers to node `zz`") && dup.message.contains("Known nodes: a"),
        "{}",
        dup.message
    );
    Ok(())
}

#[test]
fn story_copy_paste() -> Test {
    let Some(mut app) = start_or_skip("story_copy_paste")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    let a = make_box(&mut app, "rectangle", 0.0, 0.0, 100.0, 60.0, None)?;
    let b = make_box(&mut app, "ellipse", 300.0, 0.0, 100.0, 60.0, None)?;
    let arrow = text(&app.exec("element.connect", json!({ "from": a, "to": b, "label": "go" }))?, "/id")?.to_owned();
    app.exec("selection.select", json!({ "ids": [a, b, arrow] }))?;
    assert_eq!(num(&app.exec("clipboard.copy", json!({}))?, "/count")?, 4.0, "two shapes, an arrow and its label");
    let pasted = app.exec("clipboard.paste", json!({ "source": "internal", "at": [0, 500] }))?;
    let created: Vec<String> =
        list(&pasted, "/created")?.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    assert_eq!(created.len(), 4);
    assert_eq!(count(&mut app)?, 6);
    let mut rows = Vec::new();
    for id in &created {
        rows.push(row(&mut app, id)?);
    }
    let kind = |k: &str| {
        rows.iter().find(|r| r.get("kind").and_then(Value::as_str) == Some(k)).cloned().unwrap_or(Value::Null)
    };
    let (new_rect, new_oval, new_arrow) = (kind("rectangle"), kind("ellipse"), kind("arrow"));
    assert!(!new_arrow.is_null(), "an arrow was pasted");
    assert_eq!(
        bindings(&new_arrow),
        (Some(text(&new_rect, "/id")?.to_owned()), Some(text(&new_oval, "/id")?.to_owned())),
        "bound to the copies, not the originals"
    );
    assert!(![&a, &b].contains(&&text(&new_rect, "/id")?.to_owned()));

    let orig_h = num(&row(&mut app, &arrow)?, "/bounds/h")?;
    app.exec("element.move", json!({ "ids": [text(&new_oval, "/id")?], "dx": 0, "dy": 150, "snap": "none" }))?;
    assert_eq!(num(&row(&mut app, &arrow)?, "/bounds/h")?, orig_h, "the original arrow did not move");
    assert!(num(&row(&mut app, text(&new_arrow, "/id")?)?, "/bounds/h")? > 50.0, "the copied arrow followed");
    undo(&mut app, 2)?;
    assert_eq!(count(&mut app)?, 3);
    app.exec("history.redo", json!({}))?;
    assert_eq!(count(&mut app)?, 6);

    let empty = app.refusal("clipboard.paste", json!({ "source": { "text": "" } }))?;
    assert_eq!(empty.code, "clipboard_empty");
    assert!(empty.message.contains("nothing drafft.ink can paste"));
    Ok(())
}

#[test]
fn story_legacy_fixture() -> Test {
    let Some(mut app) = start_or_skip("story_legacy_fixture")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    let path = drafftink_e2e::NativeApp::fixture("legacy/all_shapes.json");
    let facts: Value = serde_json::from_str(&std::fs::read_to_string(drafftink_e2e::NativeApp::fixture(
        "legacy/all_shapes.facts.json",
    ))?)
    .map_err(|e| E2eError::Shape(e.to_string()))?;
    let opened = app.exec("doc.open", json!({ "source": { "path": path.display().to_string() } }))?;
    assert_eq!(text(&opened, "/format")?, "drafftink");
    assert_eq!(text(&opened, "/mode")?, "replace");
    assert_eq!(list(&opened, "/warnings")?.len(), 0);
    let inspect = app.exec("doc.inspect", json!({}))?;
    let counts = facts
        .pointer("/doc/counts_total")
        .and_then(Value::as_object)
        .ok_or_else(|| E2eError::Shape("no counts_total".into()))?;
    let mut shapes = 0.0;
    for (kind, n) in counts {
        let want = n.as_f64().unwrap_or_default();
        if kind == "Group" {
            assert_eq!(num(&inspect, "/groups")?, want, "groups");
        } else {
            shapes += want;
            let got =
                inspect.pointer(&format!("/kinds/{}", kind.to_lowercase())).and_then(Value::as_f64).unwrap_or(0.0);
            assert_eq!(got, want, "{kind} count");
        }
    }
    assert_eq!(num(&inspect, "/elements")?, shapes, "elements by the facts file");
    assert_eq!(inspect.pointer("/dirty"), Some(&json!(false)), "a freshly opened document is clean");
    assert_eq!(app.refusal("history.undo", json!({}))?.code, "nothing_to_undo");
    Ok(())
}

#[test]
fn story_pixels() -> Test {
    let Some(mut app) = start_or_skip("story_pixels")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    app.exec("view.set_viewport", json!({ "width": 800, "height": 600 })).ok();
    make_box(&mut app, "rectangle", 100.0, 100.0, 200.0, 120.0, Some("#ff0000"))?;
    app.exec("selection.clear", json!({}))?;
    // view.screenshot with region "all" is the shapes' bounding box, padded: the red fill fills most of it.
    let out = app.exec("view.screenshot", json!({ "region": "all", "scale": 1 }))?;
    assert_eq!((num(&out, "/width")?, num(&out, "/height")?), (200.0, 120.0));
    let shot = app.screenshot_of(&out)?;
    let red = hex("#ff0000").ok_or_else(|| E2eError::Shape("bad colour".into()))?;
    assert!(shot.share((20, 20, 160, 80), red, 24) > 0.95, "the inside is red");
    assert_eq!(shot.at(100, 60).map(|p| drafftink_e2e::close(p, red, 24)), Some(true), "the centre is red");
    // Nothing is selected, so the screenshot of the viewport differs from the shape-only one only by the page.
    let viewport = app.screenshot()?;
    assert!(
        viewport.count((0, 0, viewport.width, viewport.height), red, 24) > 1000,
        "the viewport shows the red rectangle"
    );
    Ok(())
}
