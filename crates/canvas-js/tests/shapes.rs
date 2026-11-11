//! The shape model: add, update, remove, batches, z-order, groups, text, and queries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_canvas::MarqueeMode;
use serde_json::json;

#[test]
fn shapes_are_added_updated_and_removed_by_id() {
    let mut s = stage();
    let id = s.add(rect("a", 10.0, 20.0, 100.0, 50.0)).unwrap();
    assert_eq!(id.as_str(), "a");
    assert_eq!(xywh(&s, "a"), [10.0, 20.0, 100.0, 50.0]);
    // A merge patch changes what it names and nothing else.
    s.update("a", json!({"x": 30, "style": {"stroke_width": 4}, "name": "card"})).unwrap();
    let e = s.get("a").unwrap();
    assert_eq!((e.transform.x, e.transform.y, e.style.stroke_width), (30.0, 20.0, 4.0));
    assert_eq!(e.name.as_deref(), Some("card"));
    assert_eq!(e.style.fill_color.map(|c| c.to_hex()), Some("#ffc9c9".to_owned()));
    // null removes an optional field.
    s.update("a", json!({"name": null})).unwrap();
    assert_eq!(s.get("a").unwrap().name, None);
    s.remove(&["a".to_owned()]).unwrap();
    assert!(s.is_empty());
    assert!(s.get("a").is_err());
    // Removal is hard: no tombstone stays behind, so the id can be used again.
    s.add(rect("a", 0.0, 0.0, 5.0, 5.0)).unwrap();
}

#[test]
fn ids_are_made_when_missing_and_never_collide() {
    let mut s = stage();
    s.add(json!({"id": "e1", "type": "ellipse", "w": 4, "h": 4})).unwrap();
    let made = s.add(json!({"type": "ellipse", "w": 4, "h": 4})).unwrap();
    assert_ne!(made.as_str(), "e1");
    assert_eq!(s.len(), 2);
}

#[test]
fn refusals_are_errors_with_plain_words_and_change_nothing() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 10.0, 10.0)).unwrap();
    let dup = s.add(rect("a", 5.0, 5.0, 10.0, 10.0)).unwrap_err().to_string();
    assert!(dup.contains("already exists"), "{dup}");
    let no_type = s.add(json!({"x": 1})).unwrap_err().to_string();
    assert!(no_type.contains("needs a type"), "{no_type}");
    let bad_field = s.add(json!({"type": "rectangle", "colour": "red"})).unwrap_err().to_string();
    assert!(bad_field.contains("colour"), "{bad_field}");
    let missing = s.update("zzz", json!({"x": 1})).unwrap_err().to_string();
    assert!(missing.contains("no element has the id zzz"), "{missing}");
    let retype = s.update("a", json!({"type": "ellipse"})).unwrap_err().to_string();
    assert!(retype.contains("cannot change its type"), "{retype}");
    let bad_id = s.update("not an id", json!({})).unwrap_err().to_string();
    assert!(bad_id.contains("not allowed"), "{bad_id}");
    assert_eq!(s.len(), 1);
}

#[test]
fn a_batch_commits_once_or_not_at_all() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 10.0, 10.0)).unwrap();
    let before = s.revision();
    let out = s
        .transact(json!([
            {"op": "add", "shape": rect("b", 50.0, 0.0, 10.0, 10.0)},
            {"op": "update", "id": "a", "patch": {"x": 99}},
            {"op": "group", "ids": ["a", "b"], "id": "g"},
        ]))
        .unwrap();
    assert_eq!(out, vec![json!("b"), json!(null), json!("g")]);
    assert_eq!(s.revision(), before + 1, "three steps, one commit");
    // The second step fails, so the first is undone.
    let err = s.transact(json!([
        {"op": "add", "shape": rect("c", 0.0, 0.0, 10.0, 10.0)},
        {"op": "update", "id": "nope", "patch": {"x": 1}},
    ]));
    assert!(err.is_err());
    assert!(s.get("c").is_err(), "the add was rolled back");
    assert_eq!(s.revision(), before + 1);
    assert!(s.transact(json!([{"op": "fly"}])).is_err(), "unknown operations are refused");
}

#[test]
fn z_order_groups_and_selectors() {
    let mut s = stage();
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        let mut shape = rect(id, i as f64 * 100.0, 0.0, 50.0, 50.0);
        shape["name"] = json!(if i == 1 { "card big" } else { "plain" });
        s.add(shape).unwrap();
    }
    let ids = |s: &drafftink_canvas_js::Core| s.elements().iter().map(|e| e.id.as_str().to_owned()).collect::<Vec<_>>();
    assert_eq!(ids(&s), ["a", "b", "c"]);
    s.transact(json!([{"op": "order", "ids": ["a"], "to": "front"}])).unwrap();
    assert_eq!(ids(&s), ["b", "c", "a"]);
    s.transact(json!([{"op": "order", "ids": ["a"], "to": "back"}])).unwrap();
    assert_eq!(ids(&s), ["a", "b", "c"]);
    s.transact(json!([{"op": "order", "ids": ["a"], "to": "forward"}])).unwrap();
    assert_eq!(ids(&s), ["b", "a", "c"]);
    let g = s.transact(json!([{"op": "group", "ids": ["a", "c"]}])).unwrap();
    let group = g[0].as_str().unwrap().to_owned();
    assert_eq!(s.find(&format!("group:{group}")).unwrap(), ["a", "c"]);
    assert_eq!(s.find(".card").unwrap(), ["b"]);
    assert_eq!(s.find("#c").unwrap(), ["c"]);
    assert_eq!(s.find("rectangle").unwrap().len(), 3);
    assert_eq!(s.find("*").unwrap().len(), 3);
    assert!(s.find("stage").unwrap().is_empty());
    assert!(s.find("%%").is_err());
    s.transact(json!([{"op": "ungroup", "ids": ["a"]}])).unwrap();
    assert!(s.find(&format!("group:{group}")).unwrap().is_empty());
}

#[test]
fn lines_arrows_and_strokes_take_points_relative_to_x_y() {
    let mut s = stage();
    s.add(json!({"id": "l", "type": "line", "x": 100, "y": 100, "points": [[0, 0], [50, 20], [100, 0]]})).unwrap();
    s.add(json!({"id": "ar", "type": "arrow", "x": 0, "y": 0, "points": [[-10, -10], [40, 30]], "end_head": "arrow"}))
        .unwrap();
    s.add(json!({"id": "f", "type": "freehand", "x": 0, "y": 0, "points": [[0, 0], [5, 5], [10, 0]]})).unwrap();
    assert_eq!(xywh(&s, "l"), [100.0, 100.0, 100.0, 20.0]);
    // Points may be negative: the box moves so that every point keeps its place in the world.
    assert_eq!(xywh(&s, "ar"), [-10.0, -10.0, 50.0, 40.0]);
    assert_eq!(s.get("f").unwrap().transform.w, 10.0);
    // Replacing the points of a line re-fits its box.
    s.update("l", json!({"points": [[0, 0], [10, 10]]})).unwrap();
    assert_eq!(xywh(&s, "l"), [100.0, 100.0, 10.0, 10.0]);
}

#[test]
fn text_and_math_are_sized_by_their_content() {
    let mut s = stage();
    s.add(json!({"id": "t", "type": "text", "x": 10, "y": 10, "content": "Hello\nworld"})).unwrap();
    let [_, _, w, h] = xywh(&s, "t");
    assert!(w > 20.0 && h > 30.0, "{w} x {h}");
    s.update("t", json!({"content": "Hi"})).unwrap();
    let [_, _, w2, h2] = xywh(&s, "t");
    assert!(w2 < w && h2 < h, "a shorter text is smaller: {w2} x {h2} against {w} x {h}");
    s.add(json!({"id": "m", "type": "math", "latex": "x^2 + y^2 = r^2"})).unwrap();
    let [_, _, mw, mh] = xywh(&s, "m");
    assert!(mw > 10.0 && mh > 5.0, "{mw} x {mh}");
}

#[test]
fn default_style_sits_under_the_style_of_each_shape() {
    use drafftink_canvas_js::StageOptions;
    let options: StageOptions =
        serde_json::from_value(json!({"defaultStyle": {"sloppiness": "architect", "stroke_width": 5}})).unwrap();
    let mut s = stage_with(&options);
    s.add(json!({"id": "a", "type": "rectangle", "w": 5, "h": 5})).unwrap();
    s.add(json!({"id": "b", "type": "rectangle", "w": 5, "h": 5, "style": {"stroke_width": 1}})).unwrap();
    assert_eq!(s.get("a").unwrap().style.stroke_width, 5.0);
    assert_eq!(s.get("b").unwrap().style.stroke_width, 1.0);
    assert_eq!(s.get("a").unwrap().style.sloppiness.roughness(), 0.0);
    assert_ne!(s.get("a").unwrap().style.seed, s.get("b").unwrap().style.seed, "each shape gets its own sketch seed");
}

#[test]
fn hit_testing_and_region_queries() {
    let mut s = stage();
    s.add(rect("a", -50.0, -50.0, 100.0, 100.0)).unwrap();
    s.add(rect("b", 0.0, 0.0, 100.0, 100.0)).unwrap();
    // The centre of the stage is the world origin; "b" is above "a" where they overlap.
    let hit = s.hit(400.0 + 25.0, 300.0 + 25.0, None).unwrap();
    assert_eq!((hit.id.as_str(), hit.part), ("b", "fill"));
    let all = s.hit_all(400.0 + 25.0, 300.0 + 25.0);
    assert_eq!(all.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
    assert!(s.hit(10.0, 10.0, None).is_none());
    assert_eq!(s.marquee([-60.0, -60.0, 120.0, 120.0], MarqueeMode::Contain).unwrap(), ["a"]);
    assert_eq!(s.in_rect([90.0, 90.0, 5.0, 5.0]).unwrap(), ["b"]);
    assert!(s.marquee([f64::NAN, 0.0, 1.0, 1.0], MarqueeMode::Contain).is_err());
    assert_eq!(s.screen_to_world(400.0, 300.0), [0.0, 0.0]);
    assert_eq!(s.world_to_screen(10.0, 10.0), [410.0, 310.0]);
}

#[test]
fn the_camera_pans_zooms_and_fits() {
    let mut s = stage();
    s.add(rect("a", 1000.0, 1000.0, 200.0, 100.0)).unwrap();
    s.set_camera(Some([10.0, 20.0]), Some(2.0)).unwrap();
    let c = s.camera();
    assert_eq!((c.scroll, c.zoom), ([10.0, 20.0], 2.0));
    s.set_camera(None, Some(1000.0)).unwrap();
    assert_eq!(s.camera().zoom, 30.0, "zoom is clamped");
    assert!(s.set_camera(None, Some(f64::NAN)).is_err());
    s.set_camera(Some([0.0, 0.0]), Some(1.0)).unwrap();
    s.pan_by(100.0, 50.0).unwrap();
    assert_eq!(s.camera().scroll, [-100.0, -50.0], "the content moves right, so the centre moves left");
    // Zooming keeps the world point under the cursor in place.
    let before = s.screen_to_world(200.0, 100.0);
    s.zoom_at(200.0, 100.0, 2.0).unwrap();
    let after = s.screen_to_world(200.0, 100.0);
    assert!((before[0] - after[0]).abs() < 1e-9 && (before[1] - after[1]).abs() < 1e-9);
    assert!(s.fit_to_content(40.0, None).unwrap());
    let top_left = s.world_to_screen(1000.0, 1000.0);
    assert!(top_left[0] >= 39.0 && top_left[1] >= 39.0 && top_left[0] < 400.0, "{top_left:?}");
    s.resize(1000.0, 500.0, 2.0).unwrap();
    assert!(s.resize(0.0, 500.0, 2.0).is_err());
}

#[test]
fn json_round_trip_and_svg_export() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 60.0)).unwrap();
    s.add(json!({"id": "t", "type": "text", "x": 10, "y": 80, "content": "hello <b> & co"})).unwrap();
    s.add(json!({"id": "m", "type": "math", "x": 10, "y": 120, "latex": "x^2"})).unwrap();
    let json = s.to_json(false).unwrap();
    let mut t = stage();
    t.load_bytes(json.as_bytes()).unwrap();
    assert_eq!(t.len(), 3);
    assert_eq!(xywh(&t, "a"), [0.0, 0.0, 100.0, 60.0]);
    assert!(t.hit(400.0 + 50.0, 300.0 + 30.0, None).is_some(), "the loaded scene is indexed");

    let svg = s.to_svg(&Default::default()).unwrap();
    assert!(svg.starts_with("<svg"), "{}", &svg[..40.min(svg.len())]);
    assert!(svg.contains("hello &lt;b&gt; &amp; co"), "text is escaped");
    assert!(svg.contains("<path"), "the rectangle and the formula are paths");
    // Only the selection, and it embeds only the selection.
    let one = s.to_svg(&serde_json::from_value(json!({"selection": ["a"], "embedScene": false})).unwrap()).unwrap();
    assert!(!one.contains("hello"));
    // The SVG carries the scene: opening it brings the shapes back.
    let mut u = stage();
    u.load_bytes(svg.as_bytes()).unwrap();
    assert_eq!(u.len(), 3);
    // Garbage is refused and leaves the scene alone.
    assert!(u.load_bytes(b"not a drawing").is_err());
    assert_eq!(u.len(), 3);
    assert!(stage().to_svg(&Default::default()).is_err(), "an empty scene has nothing to export");
}
