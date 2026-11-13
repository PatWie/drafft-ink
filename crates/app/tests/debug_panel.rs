//! The debug panel, end to end over a windowless shell: `Alt+D` opens it, the shell measures only while it is open,
//! it refreshes itself on a timer that exists only while it is open, and it is drawn with the palette of the theme.
//! Needs a Vulkan, Metal or DX12 adapter; without one the tests skip (`DRAFFTINK_REQUIRE_GPU=1` makes that a failure).
//! The frame time it shows leaves its own cost out: a fixed workload reads the same with the panel open as it takes with
//! the panel closed. `DRAFFTINK_SCREENSHOT_DIR=<dir>` also writes the pictures the pixel test looks at.
#![cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_app::{App, AppConfig, NoHost, Platform, SharedClipboard, WakeCell};
use drafftink_engine::ports::fake::{FakeClock, Fakes};
use drafftink_engine::services::LazyTypesetter;
use drafftink_gpu::headless::{Headless, HeadlessOptions};
use drafftink_gpu::{FrameOutput, GpuError, RedrawReason, Repaint};
use drafftink_ui::theme::Palette;
use egui::{Event, Key, Modifiers};
use serde_json::json;

const SIZE: [f32; 2] = [1100.0, 700.0];

fn platform(fakes: &Fakes) -> Platform {
    Platform {
        ports: fakes.ports(),
        host: Box::new(NoHost),
        wake: WakeCell::new(),
        clipboard: SharedClipboard::new(),
        typesetter: LazyTypesetter::shared(),
        bridge: None,
        mcp_http: None,
        is_mac: false,
    }
}

/// A windowless shell over an app, or `None` (with the skip line) on a machine without a GPU adapter.
fn shell(config: AppConfig) -> Option<Headless<App>> {
    shell_with_clock(config).map(|(shell, _)| shell)
}

/// The same, with the app's clock. It stands still until it is moved, so the panel's twice-a-second refresh is due
/// when the test says it is.
fn shell_with_clock(config: AppConfig) -> Option<(Headless<App>, FakeClock)> {
    let fakes = Fakes::new();
    let app = App::new(platform(&fakes), config);
    let options = HeadlessOptions { size_points: SIZE, scale: 1.0, touch_first: false };
    match Headless::new(app, options) {
        Ok(shell) => Some((shell, fakes.clock.clone())),
        Err(GpuError::NoAdapter(why)) => {
            if let Err(message) = drafftink_profile::skip_without_gpu(&why) {
                panic!("{message}");
            }
            None
        }
        Err(other) => panic!("a headless shell: {other}"),
    }
}

fn alt_d() -> Vec<Event> {
    let key = |pressed| Event::Key {
        key: Key::D,
        physical_key: Some(Key::D),
        pressed,
        repeat: false,
        modifiers: Modifiers::ALT,
    };
    vec![key(true), key(false)]
}

fn waits_for_the_panel(out: &FrameOutput) -> bool {
    matches!(out.repaint, Repaint::Now(RedrawReason::DebugPanel) | Repaint::After(_, RedrawReason::DebugPanel))
}

#[test]
fn the_shell_measures_and_the_panel_refreshes_only_while_the_panel_is_open() {
    let Some(mut shell) = shell(AppConfig::default()) else { return };
    shell.settle(3);
    let closed = shell.frame(Vec::new());
    assert!(!closed.stats, "a closed panel asks the shell to measure nothing");
    assert!(!waits_for_the_panel(&closed), "and keeps no timer: {:?}", closed.repaint);

    let opened = shell.frame(alt_d());
    assert!(opened.stats, "Alt+D opens the panel and the shell starts to measure");
    assert!(waits_for_the_panel(&opened), "an open panel refreshes itself: {:?}", opened.repaint);

    // The panel's first refresh, then one after the numbers come in.
    shell.settle(2);
    let open = shell.frame(Vec::new());
    assert!(open.stats && waits_for_the_panel(&open));
    assert!(
        matches!(open.repaint, Repaint::After(wait, _) if wait.as_millis() >= 400),
        "twice a second, not faster: {:?}",
        open.repaint
    );

    let shut = shell.frame(alt_d());
    assert!(!shut.stats, "Alt+D closes it again");
    assert!(!waits_for_the_panel(&shut), "and the timer goes with it: {:?}", shut.repaint);
}

#[test]
fn the_overlay_flag_opens_the_panel_at_start() {
    let Some(mut shell) = shell(AppConfig { perf_overlay: true, ..AppConfig::default() }) else { return };
    let first = shell.frame(Vec::new());
    assert!(first.stats, "`--perf-overlay` is the panel, open from the first frame");
}

/// Is a pixel the colour the palette names, within the tolerance of the alpha the panel is painted with?
fn is_near(pixel: [u8; 4], want: egui::Color32) -> bool {
    let [r, g, b, _] = want.to_array();
    let close = |got: u8, want: u8| got.abs_diff(want) <= 6;
    close(pixel[0], r) && close(pixel[1], g) && close(pixel[2], b)
}

fn near(pixel: [u8; 4], want: egui::Color32, what: &str) {
    assert!(is_near(pixel, want), "{what}: {pixel:?} is not near {want:?}");
}

#[test]
fn the_panel_is_a_card_in_the_colours_of_the_theme_in_light_and_dark() {
    for (theme, palette) in [("light", Palette::LIGHT), ("dark", Palette::DARK)] {
        let Some(mut shell) = shell(AppConfig::default()) else { return };
        shell.app_mut().session_mut().execute("settings.set", json!({ "key": "theme", "value": theme })).unwrap();
        shell.settle(3);
        // A spot inside the card, near its top right corner, where there is padding and nothing else: the strip is
        // 72 points tall, the card sits 8 below it and 12 in from the right edge, with 10 and 8 points of padding.
        let (x, y) = (SIZE[0] as u32 - 12 - 5, 72 + 8 + 4);
        shell.frame(Vec::new());
        let before = shell.screenshot().unwrap();
        assert_ne!(
            before.pixel(x, y).map(|p| (p[0], p[1], p[2])),
            Some((palette.panel.r(), palette.panel.g(), palette.panel.b()))
        );

        shell.frame(alt_d());
        shell.settle(6);
        let image = shell.screenshot().unwrap();
        near(image.pixel(x, y).unwrap(), palette.panel, &format!("{theme}: the card"));
        // The card has the border of the theme on its right edge.
        let row = 72 + 8 + 40;
        let has_border =
            (SIZE[0] as u32 - 15..=SIZE[0] as u32 - 10).any(|x| is_near(image.pixel(x, row).unwrap(), palette.border));
        assert!(has_border, "{theme}: no border-coloured pixel on the card's right edge");
        if let Ok(dir) = std::env::var("DRAFFTINK_SCREENSHOT_DIR") {
            let path = std::path::Path::new(&dir).join(format!("debug-panel-{theme}.png"));
            std::fs::create_dir_all(&dir).unwrap();
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), image.width, image.height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&image.rgba).unwrap();
        }
    }
}

/// The frame time and the panel's cost as the panel's own reading says them, in milliseconds.
fn reading(shell: &Headless<App>) -> (f64, f64) {
    let frames = shell.app().debug_view().and_then(|view| view.frames.as_ref()).expect("the panel has a reading");
    (frames.avg_ms, frames.panel_ms.expect("the panel reports its own cost"))
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

/// Pointer moves over the canvas, which draw a frame each. The same work in every round.
fn workload(shell: &mut Headless<App>, frames: u32) -> Vec<u64> {
    (0..frames)
        .map(|i| {
            let at = egui::pos2(300.0 + (i % 7) as f32 * 9.0, 300.0 + (i % 5) as f32 * 7.0);
            let started = drafftink_profile::now_ns();
            shell.frame(vec![Event::PointerMoved(at)]);
            drafftink_profile::now_ns() - started
        })
        .collect()
}

#[test]
fn a_fixed_workload_reads_the_same_frame_time_with_the_panel_open_as_it_takes_with_it_closed() {
    let Some((mut shell, clock)) = shell_with_clock(AppConfig::default()) else { return };
    for i in 0..120 {
        let bounds = json!({ "x": (i % 12) * 80, "y": (i / 12) * 60, "w": 60, "h": 40 });
        shell
            .app_mut()
            .session_mut()
            .execute("element.create", json!({ "kind": "rectangle", "bounds": bounds }))
            .unwrap();
    }
    shell.settle(8);

    // Closed and open take turns, so a machine that gets busy in between spoils both. The least of the rounds
    // is the one nothing else disturbed.
    let (mut closed, mut counted, mut open_wall, mut panel) = (f64::MAX, f64::MAX, f64::MAX, f64::MAX);
    for _ in 0..5 {
        let mut walls = workload(&mut shell, 60);
        walls.sort_unstable();
        closed = closed.min(ms(walls[walls.len() / 2]));

        shell.frame(alt_d());
        shell.settle(4);
        let mut walls = workload(&mut shell, 60);
        walls.sort_unstable();
        open_wall = open_wall.min(ms(walls[walls.len() / 2]));
        clock.advance_ms(600); // the refresh is due: the panel reads the frames it has seen
        shell.frame(Vec::new());
        let (avg, panel_ms) = reading(&shell);
        counted = counted.min(avg);
        panel = panel.min(panel_ms);
        shell.frame(alt_d());
    }
    assert!(panel > 0.0, "the panel reports a cost");
    // Counting the panel, the open panel makes the same frames read slower; leaving it out brings them back.
    let tolerance = closed * 0.15 + 0.05;
    assert!(
        (counted - closed).abs() <= tolerance,
        "counted {counted:.3} ms with the panel open, {closed:.3} ms with it closed (panel {panel:.3} ms, open frames take {open_wall:.3} ms)"
    );
    // What was left out is what the open frames take more.
    assert!(
        (counted + panel - open_wall).abs() <= open_wall * 0.15 + 0.05,
        "counted {counted:.3} + panel {panel:.3} ms is not the {open_wall:.3} ms the open frames take"
    );
}

#[test]
fn the_panel_is_rebuilt_twice_a_second_and_kept_between() {
    let Some((mut shell, clock)) = shell_with_clock(AppConfig { perf_overlay: true, ..AppConfig::default() }) else {
        return;
    };
    let frames_of = |shell: &Headless<App>| shell.app().debug_view().and_then(|v| v.frames.as_ref()).map(|f| f.frames);
    shell.settle(3);
    clock.advance_ms(600);
    shell.frame(Vec::new());
    let built = shell.app().debug_view().cloned();
    assert!(built.as_ref().is_some_and(|v| v.frames.is_some()), "a reading after the first refresh");

    shell.settle(5);
    assert_eq!(shell.app().debug_view().cloned(), built, "five frames in the same half second change nothing");

    clock.advance_ms(600);
    shell.frame(Vec::new());
    assert!(
        frames_of(&shell) > built.and_then(|v| v.frames).map(|f| f.frames),
        "the next refresh reads the new frames"
    );
}
