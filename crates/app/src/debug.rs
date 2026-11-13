//! The content of the debug panel: the numbers the shell and the renderer measured, turned into the rows the UI draws
//! (`drafftink_ui::DebugView`). Built only while the panel is open.
//!
//! Every row is something the machine reported or the renderer counted. A value that cannot be had is not shown as 0:
//! the row says why, or is left out.

use drafftink_gpu::{BackendStats, GpuMemory, GpuTime, Rasteriser, RedrawReason, RendererInfo, ShellStats, ViewInfo};
use drafftink_render::FrameReport;
use drafftink_ui::debug::{bytes_label, count_label, ms_label};
use drafftink_ui::{CollabStatus, CollabView, DebugRow, DebugSection, DebugView};

/// What the panel is made from.
pub(crate) struct DebugInputs<'a> {
    /// The renderer and its adapter.
    pub renderer: &'a RendererInfo,
    /// Frame rate, frame time and GPU numbers, once the shell has delivered them.
    pub stats: Option<&'a ShellStats>,
    /// What the last scene build drew.
    pub scene: &'a FrameReport,
    /// The surface size and scale.
    pub view: ViewInfo,
    /// Why this frame is drawn.
    pub reason: RedrawReason,
    /// The zoom of the canvas, in percent.
    pub zoom_percent: f64,
    /// The collaboration link.
    pub collab: &'a CollabView,
}

/// The panel's content.
pub(crate) fn debug_view(i: &DebugInputs<'_>) -> DebugView {
    let gpu = i.stats.map(|s| s.gpu);
    DebugView {
        frames: i.stats.map(|s| s.frames.clone()),
        sections: vec![renderer_section(i.renderer, gpu), scene_section(i), app_section(i)],
    }
}

fn renderer_section(info: &RendererInfo, gpu: Option<BackendStats>) -> DebugSection {
    let mut rows = vec![
        DebugRow::new("Renderer", info.renderer_label()),
        DebugRow::new("Adapter", if info.adapter.is_empty() { "not reported" } else { &info.adapter }),
        DebugRow::new("API", info.api.label()),
        DebugRow::new("Device", info.device.label()),
    ];
    let rasteriser = DebugRow::new("Draws on", info.rasteriser.label());
    rows.push(if info.rasteriser == Rasteriser::Cpu { rasteriser.flagged() } else { rasteriser });
    if !info.driver.is_empty() {
        rows.push(DebugRow::new("Driver", &info.driver));
    }
    if let Some(gpu) = gpu {
        rows.push(DebugRow::new("GPU time", gpu_time_label(gpu.gpu_time)));
        if let Some(GpuMemory { allocated, reserved }) = gpu.memory {
            rows.push(DebugRow::new("GPU memory", format!("{} in use", bytes_label(allocated))));
            rows.push(DebugRow::new("Reserved", bytes_label(reserved)));
        }
    }
    DebugSection { title: "Renderer", rows }
}

/// The GPU time, with what it covers. Only the composite pass is timed (see [`GpuTime`]).
fn gpu_time_label(time: GpuTime) -> String {
    match time {
        GpuTime::Unsupported => "not available here".to_owned(),
        GpuTime::Pending => "measuring…".to_owned(),
        GpuTime::Measured(ms) => format!("{} (composite)", ms_label(f64::from(ms))),
    }
}

fn scene_section(i: &DebugInputs<'_>) -> DebugSection {
    let s = i.scene;
    DebugSection {
        title: "Scene",
        rows: vec![
            DebugRow::new("Elements", count_label(s.total as u64)),
            DebugRow::new("Visible", count_label(s.in_view as u64)),
            DebugRow::new("Drawn", count_label(s.drawn as u64)),
            DebugRow::new(
                "Cache",
                format!("{} hit, {} miss", count_label(s.cache_hits as u64), count_label(s.cache_misses as u64)),
            ),
            DebugRow::new("Zoom", format!("{:.0}%", i.zoom_percent)),
        ],
    }
}

fn app_section(i: &DebugInputs<'_>) -> DebugSection {
    let [w, h] = i.view.size_px;
    DebugSection {
        title: "App",
        rows: vec![
            DebugRow::new("Redraw reason", i.reason.name()),
            DebugRow::new("Canvas", format!("{w} × {h} px at {}×", i.view.scale)),
            DebugRow::new("Collaboration", collab_label(i.collab)),
        ],
    }
}

fn collab_label(c: &CollabView) -> String {
    match &c.status {
        CollabStatus::Disconnected => "off".to_owned(),
        CollabStatus::Connecting => "connecting".to_owned(),
        CollabStatus::Connected => match c.peers.len() {
            0 => "connected, alone".to_owned(),
            1 => "connected, 1 peer".to_owned(),
            n => format!("connected, {n} peers"),
        },
        CollabStatus::Failed(_) => "failed".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use drafftink_gpu::{BackendKind, DeviceClass, GraphicsApi};
    use drafftink_profile::FrameReading;
    use drafftink_ui::{Peer, RowTone};

    use super::*;

    fn renderer(rasteriser: Rasteriser) -> RendererInfo {
        RendererInfo {
            kind: BackendKind::Vello,
            adapter: "NVIDIA L40S".into(),
            api: GraphicsApi::Vulkan,
            device: DeviceClass::Discrete,
            driver: "NVIDIA 580.1".into(),
            rasteriser,
        }
    }

    fn inputs<'a>(
        renderer: &'a RendererInfo,
        stats: Option<&'a ShellStats>,
        scene: &'a FrameReport,
        collab: &'a CollabView,
    ) -> DebugInputs<'a> {
        DebugInputs {
            renderer,
            stats,
            scene,
            view: ViewInfo { size_px: [2560, 1440], scale: 2.0 },
            reason: RedrawReason::Gesture,
            zoom_percent: 150.0,
            collab,
        }
    }

    fn row<'a>(view: &'a DebugView, section: &str, name: &str) -> Option<&'a DebugRow> {
        view.sections.iter().find(|s| s.title == section)?.rows.iter().find(|r| r.name == name)
    }

    #[test]
    fn the_panel_reports_the_scene_counts_the_renderer_gave() {
        let scene = FrameReport {
            total: 12_345,
            in_view: 800,
            drawn: 790,
            cache_hits: 700,
            cache_misses: 12,
            ..FrameReport::default()
        };
        let (r, c) = (renderer(Rasteriser::Gpu), CollabView::default());
        let v = debug_view(&inputs(&r, None, &scene, &c));
        assert_eq!(row(&v, "Scene", "Elements").map(|r| r.value.as_str()), Some("12,345"));
        assert_eq!(row(&v, "Scene", "Visible").map(|r| r.value.as_str()), Some("800"));
        assert_eq!(row(&v, "Scene", "Cache").map(|r| r.value.as_str()), Some("700 hit, 12 miss"));
        assert_eq!(row(&v, "Scene", "Zoom").map(|r| r.value.as_str()), Some("150%"));
        assert_eq!(row(&v, "App", "Redraw reason").map(|r| r.value.as_str()), Some("gesture"));
        assert_eq!(row(&v, "App", "Canvas").map(|r| r.value.as_str()), Some("2560 × 1440 px at 2×"));
    }

    #[test]
    fn without_statistics_there_are_no_frames_and_no_gpu_numbers() {
        let (r, s, c) = (renderer(Rasteriser::Gpu), FrameReport::default(), CollabView::default());
        let v = debug_view(&inputs(&r, None, &s, &c));
        assert_eq!(v.frames, None, "no reading is not a reading of zero");
        assert!(row(&v, "Renderer", "GPU time").is_none());
        assert!(row(&v, "Renderer", "GPU memory").is_none());
        assert_eq!(row(&v, "Renderer", "Adapter").map(|r| r.value.as_str()), Some("NVIDIA L40S"));
    }

    #[test]
    fn gpu_numbers_are_shown_only_when_the_device_gave_them() {
        let (r, s, c) = (renderer(Rasteriser::Gpu), FrameReport::default(), CollabView::default());
        let stats = |gpu| ShellStats { frames: FrameReading::default(), gpu };
        let none = stats(BackendStats { gpu_time: GpuTime::Unsupported, memory: None });
        let v = debug_view(&inputs(&r, Some(&none), &s, &c));
        assert_eq!(row(&v, "Renderer", "GPU time").map(|r| r.value.as_str()), Some("not available here"));
        assert!(row(&v, "Renderer", "GPU memory").is_none(), "no allocator report, no memory row");

        let full = stats(BackendStats {
            gpu_time: GpuTime::Measured(0.42),
            memory: Some(GpuMemory { allocated: 812 * 1024, reserved: 3 << 30 }),
        });
        let v = debug_view(&inputs(&r, Some(&full), &s, &c));
        assert_eq!(row(&v, "Renderer", "GPU time").map(|r| r.value.as_str()), Some("0.42 ms (composite)"));
        assert_eq!(row(&v, "Renderer", "GPU memory").map(|r| r.value.as_str()), Some("812 KiB in use"));
        assert_eq!(row(&v, "Renderer", "Reserved").map(|r| r.value.as_str()), Some("3.0 GiB"));

        let pending = stats(BackendStats { gpu_time: GpuTime::Pending, memory: None });
        let v = debug_view(&inputs(&r, Some(&pending), &s, &c));
        assert_eq!(row(&v, "Renderer", "GPU time").map(|r| r.value.as_str()), Some("measuring…"));
    }

    #[test]
    fn a_software_rasteriser_is_flagged_and_a_gpu_is_not() {
        let (s, c) = (FrameReport::default(), CollabView::default());
        let gpu = renderer(Rasteriser::Gpu);
        let v = debug_view(&inputs(&gpu, None, &s, &c));
        assert_eq!(row(&v, "Renderer", "Draws on").map(|r| r.tone), Some(RowTone::Normal));
        let cpu = renderer(Rasteriser::Cpu);
        let v = debug_view(&inputs(&cpu, None, &s, &c));
        assert_eq!(row(&v, "Renderer", "Draws on").map(|r| r.tone), Some(RowTone::Warn));
    }

    #[test]
    fn a_browser_that_hides_the_adapter_and_the_driver_leaves_those_rows_honest() {
        let (s, c) = (FrameReport::default(), CollabView::default());
        let hidden = RendererInfo {
            adapter: String::new(),
            driver: String::new(),
            device: DeviceClass::Unknown,
            ..renderer(Rasteriser::Gpu)
        };
        let v = debug_view(&inputs(&hidden, None, &s, &c));
        assert_eq!(row(&v, "Renderer", "Adapter").map(|r| r.value.as_str()), Some("not reported"));
        assert_eq!(row(&v, "Renderer", "Device").map(|r| r.value.as_str()), Some("not reported"));
        assert!(row(&v, "Renderer", "Driver").is_none());
    }

    #[test]
    fn collaboration_reads_as_words() {
        let peer = |id: &str| Peer {
            id: id.into(),
            name: id.into(),
            color: "#fff".into(),
            has_cursor: false,
            followed: false,
        };
        let mut c = CollabView::default();
        assert_eq!(collab_label(&c), "off");
        c.status = CollabStatus::Connected;
        assert_eq!(collab_label(&c), "connected, alone");
        c.peers = vec![peer("a")];
        assert_eq!(collab_label(&c), "connected, 1 peer");
        c.peers.push(peer("b"));
        assert_eq!(collab_label(&c), "connected, 2 peers");
    }
}
