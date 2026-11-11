//! Reads the overlay data out of the output of a gesture frame.
//!
//! A live frame of `element.move`, `element.resize`, `element.rotate` and `arrow.move_point` returns, next to its
//! result, what the overlay needs (`docs/commands.md` section 1.10 rule 3): `guides`, `frame_highlight`,
//! `bind_highlight`, `size_badge`, `angle_badge`. [`apply`] copies them into [`OverlayParts`].

use drafftink_geom::guides::{SmartGuide, SmartGuideKind};
use drafftink_geom::kurbo::Point;
use drafftink_scene::ElementId;
use serde_json::Value;

use super::overlay::{AngleRays, Badge, OverlayParts};

fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn guide(v: &Value) -> Option<SmartGuide> {
    let kind = match v.get("kind")?.as_str()? {
        "vertical" => SmartGuideKind::Vertical,
        "horizontal" => SmartGuideKind::Horizontal,
        "equal_spacing_h" => SmartGuideKind::EqualSpacingH,
        "equal_spacing_v" => SmartGuideKind::EqualSpacingV,
        _ => return None,
    };
    Some(SmartGuide {
        kind,
        position: number(v, "position")?,
        start: number(v, "start")?,
        end: number(v, "end")?,
        snap_points: v
            .get("snap_points")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default(),
        gap: v.get("gap").and_then(Value::as_array).and_then(|g| Some((g.first()?.as_f64()?, g.get(1)?.as_f64()?))),
    })
}

fn badge(v: &Value) -> Option<Badge> {
    if let Some(a) = v.get("angle_badge").filter(|a| !a.is_null()) {
        let c = a.get("center")?.as_array()?;
        let center = Point::new(c.first()?.as_f64()?, c.get(1)?.as_f64()?);
        let snapped = a.get("snapped").and_then(Value::as_bool).unwrap_or(false);
        return Some(Badge::Angle { center, degrees: number(a, "degrees")?, snapped });
    }
    let s = v.get("size_badge").filter(|s| !s.is_null())?;
    Some(Badge::Size { w: number(s, "w")?, h: number(s, "h")? })
}

fn point(v: &Value) -> Option<Point> {
    let p = v.as_array()?;
    Some(Point::new(p.first()?.as_f64()?, p.get(1)?.as_f64()?))
}

fn angle_rays(v: &Value) -> Option<AngleRays> {
    let a = v.get("angle_snap").filter(|a| !a.is_null())?;
    Some(AngleRays {
        origin: point(a.get("origin")?)?,
        step_degrees: number(a, "step")?,
        locked_degrees: number(a, "degrees")?,
    })
}

fn highlight(v: &Value) -> Vec<ElementId> {
    ["frame_highlight", "bind_highlight"]
        .into_iter()
        .filter_map(|k| v.get(k)?.as_str())
        .filter_map(|id| ElementId::new(id).ok())
        .collect()
}

/// Copies guides, highlights and badge of a frame output into the overlay parts. Returns whether the parts
/// changed, so that the caller can ask for an overlay redraw only then.
pub(crate) fn apply(parts: &mut OverlayParts, frame: &Value) -> bool {
    let guides: Vec<SmartGuide> =
        frame.get("guides").and_then(Value::as_array).map(|a| a.iter().filter_map(guide).collect()).unwrap_or_default();
    let highlight = highlight(frame);
    let badge = badge(frame);
    let rays = angle_rays(frame);
    let snap_point = frame.get("snapped_to").and_then(point);
    let changed = parts.guides != guides
        || parts.highlight != highlight
        || parts.badge != badge
        || parts.angle_rays != rays
        || parts.snap_point != snap_point;
    parts.guides = guides;
    parts.highlight = highlight;
    parts.badge = badge;
    parts.angle_rays = rays;
    parts.snap_point = snap_point;
    changed
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_frame_output_fills_guides_highlight_and_badge() {
        let mut parts = OverlayParts::default();
        let frame = json!({
            "guides": [
                { "kind": "vertical", "position": 5.0, "start": 0.0, "end": 9.0, "snap_points": [1.0, 4.0], "gap": [2.0, 7.0] },
                { "kind": "nonsense" },
            ],
            "frame_highlight": "f1",
            "angle_badge": { "center": [1.0, 2.0], "degrees": 45.0, "snapped": true },
        });
        assert!(apply(&mut parts, &frame));
        assert_eq!(parts.guides.len(), 1);
        assert_eq!(parts.guides[0].snap_points, vec![1.0, 4.0], "the markers of move and resize guides survive");
        assert_eq!(parts.guides[0].gap, Some((2.0, 7.0)));
        assert_eq!(parts.highlight, vec![ElementId::new("f1").unwrap()]);
        assert_eq!(parts.badge, Some(Badge::Angle { center: Point::new(1.0, 2.0), degrees: 45.0, snapped: true }));
        assert!(!apply(&mut parts, &frame), "the same frame again changes nothing");
        assert!(apply(&mut parts, &json!({})));
        assert!(parts.guides.is_empty() && parts.highlight.is_empty() && parts.badge.is_none());
    }

    #[test]
    fn hostile_output_is_ignored() {
        let mut parts = OverlayParts::default();
        assert!(!apply(&mut parts, &json!({ "guides": 7, "frame_highlight": 3, "size_badge": "x" })));
    }

    #[test]
    fn a_point_frame_fills_the_angle_rays_and_the_snap_point() {
        let mut parts = OverlayParts::default();
        let frame = json!({
            "snapped_to": [3.0, 4.0],
            "angle_snap": { "origin": [1.0, 1.0], "degrees": 45.0, "step": 15.0 },
        });
        assert!(apply(&mut parts, &frame));
        assert_eq!(parts.snap_point, Some(Point::new(3.0, 4.0)));
        assert_eq!(
            parts.angle_rays,
            Some(AngleRays { origin: Point::new(1.0, 1.0), step_degrees: 15.0, locked_degrees: 45.0 })
        );
        assert!(!apply(&mut parts, &frame));
        assert!(apply(&mut parts, &json!({})));
        assert!(parts.snap_point.is_none() && parts.angle_rays.is_none());
    }
}
