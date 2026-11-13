//! The debug panel: frame rate and frame time, the renderer and the GPU, the scene. It floats at the top right, under
//! the strip, drawn with the same card, type scale and palette as the other panels. The content is the app's
//! ([`crate::debug::DebugView`]); this file only lays it out.

use drafftink_profile::FrameReading;
use egui::{
    Align, Align2, Area, Context, Id, Layout, Order, Pos2, Rect, RichText, Role, Sense, Shape, Stroke, Ui, WidgetInfo,
    pos2, vec2,
};

use crate::action::Verb;
use crate::chrome::GAP;
use crate::debug::{DebugRow, DebugSection, RowTone, ms_label};
use crate::icons::Icon;
use crate::state::UiState;
use crate::surfaces::collab::dot;
use crate::theme::{MARGIN, Palette, SMALL_BUTTON, TextSize, radius, space};
use crate::widgets::{Cx, panel_frame};

/// Width of the card.
const WIDTH: f32 = 264.0;
/// Height of the frame-time graph.
const GRAPH_HEIGHT: f32 = 36.0;
/// The frame time of 60 frames per second, the line the graph is read against.
const BUDGET_60_MS: f32 = 16.7;
/// The graph's scale never goes below this, so a quiet history does not look like a spike.
const GRAPH_MIN_MS: f32 = 2.0 * BUDGET_60_MS;
/// Diameter of the dot before a value that is worth a look.
const WARN_DOT: f32 = 8.0;
/// A mean frame time above this is slower than 60 frames per second.
const SLOW_FRAME_MS: f64 = BUDGET_60_MS as f64;

/// Draws the panel while it is open (also in zen mode: it is a diagnostic, not interface chrome).
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !st.debug_open {
        return;
    }
    let pal = cx.pal;
    let screen = ctx.content_rect();
    let top = st.chrome.below_strip(screen) + GAP;
    Area::new(Id::unique("debug_panel")).anchor(Align2::RIGHT_TOP, vec2(-MARGIN, top)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&pal).show(ui, |ui| {
                ui.set_width(WIDTH);
                header(ui, cx);
                match cx.model.debug.as_ref() {
                    None => {
                        ui.label(muted(&pal, "Measuring…"));
                    }
                    Some(view) => {
                        frames(ui, &pal, view.frames.as_ref());
                        for section in &view.sections {
                            rows(ui, &pal, section);
                        }
                    }
                }
            });
        },
    );
}

fn muted(pal: &Palette, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(TextSize::Small.points()).color(pal.muted)
}

fn caption(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).font(TextSize::Label.hand()).color(pal.muted));
}

/// The title and the close button.
fn header(ui: &mut Ui, cx: &mut Cx<'_>) {
    let pal = cx.pal;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Debug").font(TextSize::Label.hand()).color(pal.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            cx.verb_button(ui, Verb::ToggleDebug, Icon::Close, SMALL_BUTTON, "Close the debug panel", false);
        });
    });
}

/// One value cell: a dot before it when it is worth a look. Long values are cut and show in full as the tooltip.
fn value(ui: &mut Ui, pal: &Palette, row: &DebugRow) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::SM;
        if row.tone == RowTone::Warn {
            dot(ui, pal.warn, WARN_DOT);
        }
        let text = RichText::new(&row.value).size(TextSize::Small.points()).color(pal.text);
        ui.add(egui::Label::new(text).truncate()).on_hover_text(&row.value);
    });
}

fn grid(ui: &mut Ui, pal: &Palette, id: &str, rows: &[DebugRow]) {
    egui::Grid::new(Id::unique(("debug_grid", id))).num_columns(2).spacing(vec2(12.0, 3.0)).show(ui, |ui| {
        for row in rows {
            ui.label(muted(pal, row.name));
            value(ui, pal, row);
            ui.end_row();
        }
    });
}

fn rows(ui: &mut Ui, pal: &Palette, section: &DebugSection) {
    ui.add_space(space::MD);
    caption(ui, pal, section.title);
    grid(ui, pal, section.title, &section.rows);
}

/// The frame rate, the frame time and the graph.
fn frames(ui: &mut Ui, pal: &Palette, reading: Option<&FrameReading>) {
    caption(ui, pal, "Frames");
    let Some(reading) = reading else {
        ui.label(muted(pal, "Measuring…"));
        return;
    };
    grid(ui, pal, "frames", &frame_rows(reading));
    graph(ui, pal, reading);
}

/// The rows of the frame section. The app draws on demand, so a quiet canvas is `idle`, not 0 frames per second.
pub(crate) fn frame_rows(r: &FrameReading) -> Vec<DebugRow> {
    let mut rows = Vec::new();
    if r.idle {
        rows.push(DebugRow::new("Rate", "idle (draws on demand)"));
        if let (Some(last), Some(since)) = (r.last_ms, r.since_last_ms) {
            rows.push(DebugRow::new("Last frame", format!("{}, {}", ms_label(last), ago_label(since))));
        }
    } else {
        rows.push(DebugRow::new("Rate", format!("{} fps", r.frames)));
        let time = format!("avg {}, worst {}", ms_label(r.avg_ms), ms_label(r.worst_ms));
        let row = DebugRow::new("Frame time", time);
        rows.push(if r.avg_ms > SLOW_FRAME_MS { row.flagged() } else { row });
    }
    rows
}

/// How long ago, in words: `0.4 s ago`, `12 s ago`, `3 min ago`.
fn ago_label(ms: f64) -> String {
    let seconds = ms / 1000.0;
    if seconds < 10.0 {
        format!("{seconds:.1} s ago")
    } else if seconds < 120.0 {
        format!("{seconds:.0} s ago")
    } else {
        format!("{:.0} min ago", seconds / 60.0)
    }
}

/// The frame times of the latest frames as a line, against the 60 fps budget. After a quiet time the line is the last
/// burst of frames, drawn in the muted colour.
fn graph(ui: &mut Ui, pal: &Palette, r: &FrameReading) {
    if r.history_ms.is_empty() {
        return;
    }
    ui.add_space(space::SM);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), GRAPH_HEIGHT), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, "Frame time history"));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, radius::SM, pal.button);
    let scale = r.history_ms.iter().copied().fold(GRAPH_MIN_MS, f32::max);
    let y_of = |ms: f32| rect.bottom() - (ms / scale).clamp(0.0, 1.0) * rect.height();
    let budget = y_of(BUDGET_60_MS);
    painter.hline(rect.x_range(), budget, Stroke::new(1.0, pal.separator));
    let points = line_points(rect, &r.history_ms, y_of);
    let color = if r.idle { pal.muted } else { pal.accent };
    painter.add(Shape::line(points, Stroke::new(1.5, color)));
    painter.text(
        pos2(rect.left() + space::SM, budget - 1.0),
        Align2::LEFT_BOTTOM,
        "16.7 ms",
        TextSize::Micro.font(),
        pal.muted,
    );
    response.on_hover_text(format!("Latest {} frames. The line is 60 frames per second.", r.history_ms.len()));
}

/// The graph's points: the oldest frame on the left, the newest on the right edge.
fn line_points(rect: Rect, history: &[f32], y_of: impl Fn(f32) -> f32) -> Vec<Pos2> {
    let steps = history.len().saturating_sub(1).max(1) as f32;
    let width = rect.width() - 2.0 * space::SM;
    history
        .iter()
        .enumerate()
        .map(|(i, ms)| pos2(rect.left() + space::SM + width * i as f32 / steps, y_of(*ms)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(frames: u32, avg: f64, worst: f64) -> FrameReading {
        FrameReading { frames, avg_ms: avg, worst_ms: worst, idle: frames == 0, ..FrameReading::default() }
    }

    #[test]
    fn a_busy_canvas_shows_the_rate_and_the_frame_time() {
        let rows = frame_rows(&reading(58, 2.1, 6.3));
        assert_eq!(rows[0], DebugRow::new("Rate", "58 fps"));
        assert_eq!(rows[1], DebugRow::new("Frame time", "avg 2.10 ms, worst 6.30 ms"));
        assert_eq!(rows[1].tone, RowTone::Normal);
    }

    #[test]
    fn slow_frames_are_marked() {
        let rows = frame_rows(&reading(20, 24.0, 40.0));
        assert_eq!(rows[1].tone, RowTone::Warn);
    }

    #[test]
    fn a_quiet_canvas_is_idle_and_not_zero_fps() {
        let mut r = reading(0, 0.0, 0.0);
        r.last_ms = Some(3.2);
        r.since_last_ms = Some(4_200.0);
        let rows = frame_rows(&r);
        assert_eq!(rows[0], DebugRow::new("Rate", "idle (draws on demand)"));
        assert_eq!(rows[0].tone, RowTone::Normal, "idle is not a fault");
        assert_eq!(rows[1], DebugRow::new("Last frame", "3.20 ms, 4.2 s ago"));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn idle_before_any_frame_has_no_last_frame_row() {
        assert_eq!(frame_rows(&reading(0, 0.0, 0.0)).len(), 1);
    }

    #[test]
    fn the_age_of_a_frame_is_in_the_unit_that_fits() {
        assert_eq!(ago_label(400.0), "0.4 s ago");
        assert_eq!(ago_label(12_300.0), "12 s ago");
        assert_eq!(ago_label(180_000.0), "3 min ago");
    }

    #[test]
    fn the_graph_runs_from_the_left_edge_to_the_right_one() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 40.0));
        let points = line_points(rect, &[1.0, 2.0, 3.0], |_| 0.0);
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].x, space::SM);
        assert_eq!(points[2].x, 100.0 - space::SM);
        assert_eq!(line_points(rect, &[1.0], |_| 0.0).len(), 1, "one frame is one point and no division by zero");
    }
}
