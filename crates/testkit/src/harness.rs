//! [`Harness`]: a [`Session`] over fake ports and the real typesetter, with deterministic ids and clock.
//!
//! **The rule.** The harness has a method only for what a command cannot do. Everything a user can do is a
//! command, and a test runs it with `harness.session.execute(..)` (scenarios: an `execute` step). A method here
//! exists because the thing it does is not something a user does: reading a file off the test disk, moving a fake
//! clock, rendering pixels for a probe, looking inside the scene where no read-only command can yet.
//!
//! Every public method that takes `self` therefore carries a doc line that starts with `Reason:`. The accounting
//! test (`tests/accounting.rs`, run by `cargo xtask harness`) fails when
//!
//! * a method has no `Reason:` line,
//! * a method is declared but no scenario and no integration test calls it (delete it), or
//! * a scenario step or an integration test calls a harness method that is not declared.
//!
//! Methods return JSON ([`serde_json::Value`]) so a scenario can assert on the result with the same path syntax it
//! uses for command output. The dispatcher that maps a scenario's `"harness": "<name>"` to a method is in
//! [`crate::scenario`]; the accounting test checks that it knows every method here.
//!
//! **Planned, not here yet.** `pointer_script`: feed platform-neutral pointer events through the recognizers of
//! `drafftink-canvas` into the engine. It waits for the intent-to-command controller (it will live in
//! `drafftink-engine`). Until then no test can drive a drag with a pointer; gestures are tested as commands
//! (`gesture.begin`, `gesture.update`, `gesture.end`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use drafftink_engine::Session;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::{LazyTypesetter, Services, SharedTypesetter};
use drafftink_engine::store::TabDoc;
use drafftink_scene::{Document, Element, ElementId, ElementKind, Kind, SceneRead};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::scenegen;

/// Why a harness call failed. The text is plain words and names what to change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HarnessError {
    /// A path argument that is absolute or climbs out of the fixture directory.
    #[error(
        "`{0}` is not a path inside tests/fixtures. Use a relative path without `..`, for example `legacy/welcome.json`."
    )]
    BadPath(String),
    /// A fixture that cannot be read.
    #[error(
        "cannot read fixture `{path}`: {why}. Fixtures live in tests/fixtures; `cargo xtask fixtures` explains how they are made."
    )]
    Unreadable {
        /// The path as given.
        path: String,
        /// What the file system said.
        why: String,
    },
    /// Bytes that the io crate cannot open as a document.
    #[error("cannot open `{path}` as a document: {why}")]
    NotADocument {
        /// The path as given.
        path: String,
        /// What the importer said.
        why: String,
    },
    /// The scene crate rejected the content.
    #[error("the scene store refused the document: {0}")]
    Store(String),
    /// A scene probe asked for an element that is not there.
    #[error(
        "no element `{0}` in the active tab. Probe without `id` to see the counts, or list ids with `doc.outline`."
    )]
    NoSuchElement(String),
    /// Arguments of a harness call that do not fit its signature.
    #[error("bad arguments for harness method `{method}`: {why}")]
    BadArgs {
        /// The method.
        method: String,
        /// What is wrong.
        why: String,
    },
    /// There is no GPU adapter and the call needs one.
    #[error(
        "no GPU adapter on this machine ({0}). Set DRAFFTINK_REQUIRE_GPU=1 to make this a failure on a machine that must have one."
    )]
    NoGpu(String),
    /// The GPU path failed after the device was created.
    #[error("rendering for the probe failed: {0}")]
    Render(String),
}

/// Result of a harness call.
pub type HarnessResult = Result<Value, HarnessError>;

/// The headless test rig. See the module docs for the rule about methods.
pub struct Harness {
    /// The session under test. Run commands on it: `h.session.execute("view.zoom_in", json!({}))`.
    pub session: Session,
    /// The fake ports the session was built with, to inspect what it wrote (clipboard, storage, files).
    pub fakes: Fakes,
    typesetter: SharedTypesetter,
    fixtures: PathBuf,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

/// The repository's `tests/fixtures` directory.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

impl Harness {
    /// A session with every fake port (clipboard, storage, files, renderer, net), sequential ids `e1`, `e2`, ...
    /// and a clock that only moves when a test says so. Text and formulas are measured by the real typesetter.
    pub fn new() -> Self {
        let fakes = Fakes::new();
        let ports = fakes.ports();
        Self::build(fakes, ports)
    }

    /// Like [`Self::new`] but with only the clock and ids, as a shell without a clipboard, storage or renderer
    /// has. Commands that need a missing port must refuse with `port_unavailable`.
    pub fn headless() -> Self {
        let fakes = Fakes::new();
        let ports = fakes.headless_ports();
        Self::build(fakes, ports)
    }

    fn build(fakes: Fakes, ports: drafftink_engine::Ports) -> Self {
        let typesetter = LazyTypesetter::shared();
        let session = Session::with_services(ports, Services::sharing(SharedTypesetter::clone(&typesetter)));
        Self { session, fakes, typesetter, fixtures: fixtures_dir() }
    }

    /// Puts `doc` into the active tab as a fresh document: no undo history, nothing selected, saved.
    fn install(&mut self, doc: &Document) -> Result<(), HarnessError> {
        let td = TabDoc::from_document(doc).map_err(|e| HarnessError::Store(e.to_string()))?;
        let tab = self.session.tab_mut();
        tab.saved_revision = td.revision();
        tab.doc = td;
        tab.observed = drafftink_engine::state::Observed::default();
        Ok(())
    }

    /// Opens `tests/fixtures/<path>` (native, old drafft-ink, Excalidraw, gzip) as the active tab's document.
    /// Output: `{path, format, elements, warnings}`.
    ///
    /// Reason: a file on the test machine's disk is outside every port. `doc.open` reads through the `Files` port,
    /// and the fake keeps files in memory, so it cannot see `tests/fixtures`. Once `doc.open` is registered this
    /// method should copy the bytes into the fake `Files` port and run that command instead.
    pub fn load_fixture(&mut self, path: &str) -> HarnessResult {
        let rel = Path::new(path);
        let climbs = rel.components().any(|c| !matches!(c, std::path::Component::Normal(_)));
        if path.is_empty() || climbs {
            return Err(HarnessError::BadPath(path.to_owned()));
        }
        let full = self.fixtures.join(rel);
        let bytes =
            std::fs::read(&full).map_err(|e| HarnessError::Unreadable { path: path.to_owned(), why: e.to_string() })?;
        let opened = {
            let s = &mut self.session;
            drafftink_io::open(&bytes, &mut *s.ports.ids, s.services.io())
                .map_err(|e| HarnessError::NotADocument { path: path.to_owned(), why: e.to_string() })?
        };
        let drafftink_io::Opened::Document { imported, source } = opened else {
            return Err(HarnessError::NotADocument {
                path: path.to_owned(),
                why: "it is a library or a diagram, not a document".to_owned(),
            });
        };
        let warnings = imported.report.warnings.clone();
        self.install(&imported.document)?;
        Ok(json!({
            "path": path,
            "format": format!("{source:?}"),
            "elements": self.session.tab().doc.store().len(),
            "warnings": warnings,
        }))
    }

    /// Replaces the active tab's document with the generated scene of [`crate::scenegen`] (`count` elements, `seed`).
    /// Output: `{elements, seed}`.
    ///
    /// Reason: a scene of 1k to 100k elements cannot be made with commands in reasonable time, and the scale
    /// scenarios and benches must all start from the same scene. The generator is deterministic.
    pub fn load_generated(&mut self, count: usize, seed: u64) -> HarnessResult {
        let doc = scenegen::generate(count, seed).map_err(|e| HarnessError::Store(e.to_string()))?;
        self.install(&doc)?;
        Ok(json!({ "elements": self.session.tab().doc.store().len(), "seed": seed }))
    }

    /// Moves the fake wall and monotonic clocks forward by `ms` milliseconds. Output: `{now_ms}`.
    ///
    /// Reason: time is a port, and a command cannot move a fake clock (the real one moves on its own). Undo
    /// coalescing windows, autosave and the recent-command ring depend on elapsed time.
    pub fn advance_clock(&mut self, ms: u64) -> HarnessResult {
        self.fakes.clock.advance_ms(ms);
        Ok(json!({ "now_ms": drafftink_engine::ports::Clock::now_ms(&self.fakes.clock) }))
    }

    /// Looks inside the active tab's scene. Without `id`: `{total, kinds, groups, bound_arrows, bounds}`.
    /// With `id`: also `element`, that element's `{id, kind, name, bounds, angle, group_ids, frame, bindings,
    /// locked}`, its `center`, its `points` in world coordinates (lines and arrows) and its `binding` records. With `outline`:
    /// also `outline_text`, the text form of the document outline.
    ///
    /// Reason: no read-only command lists elements yet. `element.query` and `doc.outline` (docs/commands.md) will
    /// answer all of this from the same `drafftink-edit::query` functions this method calls. When they are
    /// registered, scenarios should move to them, and this method goes away with its last caller.
    pub fn scene_probe(&self, id: Option<&str>, outline: bool) -> HarnessResult {
        use drafftink_edit::query::{self, Field, OutlineOpts};
        let store = self.session.tab().doc.store();
        let mut kinds = std::collections::BTreeMap::<&'static str, usize>::new();
        let (mut groups, mut bound_arrows) = (std::collections::BTreeSet::new(), 0usize);
        let mut bounds: Option<drafftink_geom::kurbo::Rect> = None;
        for e in store.iter() {
            *kinds.entry(e.tag().name()).or_insert(0) += 1;
            groups.extend(e.group_ids.iter().cloned());
            if e.tag() == Kind::Arrow && e.binding_targets().iter().any(Option::is_some) {
                bound_arrows += 1;
            }
            let b = query::element_bounds(e);
            bounds = Some(bounds.map_or(b, |u| u.union(b)));
        }
        let mut out = json!({
            "total": store.len(),
            "kinds": kinds,
            "groups": groups.len(),
            "bound_arrows": bound_arrows,
            "bounds": bounds.map(|b| json!({ "x": b.x0, "y": b.y0, "w": b.width(), "h": b.height() })),
        });
        let Value::Object(map) = &mut out else { return Ok(out) };
        if let Some(raw) = id {
            let eid = ElementId::new(raw).map_err(|_| HarnessError::NoSuchElement(raw.to_owned()))?;
            let el =
                store.get(&eid).filter(|e| !e.deleted).ok_or_else(|| HarnessError::NoSuchElement(raw.to_owned()))?;
            let fields = [
                Field::Id,
                Field::Kind,
                Field::Name,
                Field::Bounds,
                Field::Angle,
                Field::GroupIds,
                Field::Frame,
                Field::Bindings,
                Field::Locked,
            ];
            let mut described = query::describe(store, &eid, &fields);
            if let Value::Object(d) = &mut described {
                let c = el.transform.center();
                d.insert("center".to_owned(), json!([c.x, c.y]));
                d.insert("points".to_owned(), world_points_json(el));
                d.insert("binding".to_owned(), binding_json(el));
            }
            map.insert("element".to_owned(), described);
        }
        if outline {
            let text = query::outline(store, &OutlineOpts::default()).to_text();
            map.insert("outline_text".to_owned(), Value::String(text));
        }
        Ok(out)
    }

    /// Renders the active tab's current view offscreen (Vello through `drafftink-render` and `drafftink-gpu`) and
    /// reports pixels. See [`PixelArgs`]. Output: `{adapter, software, hardware, size, ink_px, pixels}`.
    ///
    /// Reason: pixels exist only on a GPU, outside every port. `view.screenshot` goes through the `Renderer`
    /// port, which a headless session may lack, and returns a PNG; a probe needs counts and single pixels at
    /// once, and must say whether the adapter was hardware or software.
    ///
    /// Without an adapter this returns [`HarnessError::NoGpu`]; scenarios avoid that by declaring
    /// `"requires": {"gpu": true}`, which skips them (or fails them when `DRAFFTINK_REQUIRE_GPU=1`).
    pub fn pixel_probe(&mut self, args: &PixelArgs) -> HarnessResult {
        let (w, h) = (args.size[0].clamp(1, 4096), args.size[1].clamp(1, 4096));
        let view = {
            let v = self.session.view();
            drafftink_render::View::new(
                v.zoom,
                drafftink_geom::kurbo::Vec2::new(
                    f64::from(w) / 2.0 - v.scroll.x * v.zoom,
                    f64::from(h) / 2.0 - v.scroll.y * v.zoom,
                ),
                drafftink_geom::kurbo::Size::new(f64::from(w), f64::from(h)),
            )
        };
        let store = self.session.tab().doc.store();
        let index = drafftink_render::build_paint_index(store);
        let mut composer = drafftink_render::SceneComposer::new();
        composer.set_options(drafftink_render::RenderOptions::for_export(drafftink_render::Theme::Light));
        let mut scene = drafftink_render::Scene::new();
        self.typesetter.with(|ts| composer.compose(store, &index, &view, ts, &mut scene));
        let base = composer.background(store);

        let mut guard = gpu_state().map_err(HarnessError::NoGpu)?;
        let st = &mut *guard;
        let image = st
            .compositor
            .render_to_image_blocking(&st.gpu, [w, h], Some((&scene, base)), None)
            .map_err(|e| HarnessError::Render(e.to_string()))?;
        let background = image.pixel(0, 0).unwrap_or([0, 0, 0, 0]);
        let ink_px = image.rgba.chunks_exact(4).filter(|p| p != &&background[..]).count();
        let mut pixels = Vec::with_capacity(args.points.len());
        for p in &args.points {
            let [sx, sy] = match p {
                ProbePoint::Screen { screen } => *screen,
                ProbePoint::World { world } => {
                    let q = view.world_to_device() * drafftink_geom::kurbo::Point::new(world[0], world[1]);
                    [q.x, q.y]
                }
            };
            let at = |v: f64| if v.is_finite() && v >= 0.0 { v.floor() as u32 } else { u32::MAX };
            pixels.push(image.pixel(at(sx), at(sy)).map_or(Value::Null, |px| json!(px)));
        }
        Ok(json!({
            "adapter": st.summary,
            "software": st.software,
            "hardware": !st.software,
            "size": [w, h],
            "background": background,
            "ink_px": ink_px,
            "pixels": pixels,
        }))
    }
}

/// Arguments of [`Harness::pixel_probe`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelArgs {
    /// Image size in pixels. The view is the tab's view (zoom, scroll) around the image centre.
    #[serde(default = "default_size")]
    pub size: [u32; 2],
    /// Where to read single pixels. `ink_px` (pixels unlike the one at (0, 0)) is always reported.
    #[serde(default)]
    pub points: Vec<ProbePoint>,
}

fn default_size() -> [u32; 2] {
    [400, 300]
}

/// A place to read a pixel.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ProbePoint {
    /// `{"screen": [x, y]}`: pixel coordinates in the probe image.
    Screen {
        /// x, y in pixels.
        screen: [f64; 2],
    },
    /// `{"world": [x, y]}`: a world point, mapped through the view.
    World {
        /// x, y in world units.
        world: [f64; 2],
    },
}

fn world_points_json(e: &Element) -> Value {
    drafftink_edit::linear::world_points(e)
        .map_or(Value::Null, |pts| Value::Array(pts.iter().map(|p| json!([p.x, p.y])).collect()))
}

fn binding_json(e: &Element) -> Value {
    let one = |b: &Option<drafftink_scene::Binding>| {
        b.as_ref().map_or(Value::Null, |b| {
            json!({ "target": b.target, "mode": format!("{:?}", b.mode).to_lowercase(), "fixed_point": b.fixed_point })
        })
    };
    match &e.kind {
        ElementKind::Line(d) | ElementKind::Arrow(d) => {
            json!({ "start": one(&d.start_binding), "end": one(&d.end_binding) })
        }
        _ => Value::Null,
    }
}

// ---------------------------------------------------------------------------------------------
// The GPU
// ---------------------------------------------------------------------------------------------

struct GpuState {
    gpu: drafftink_gpu::Gpu,
    compositor: drafftink_gpu::Compositor,
    summary: String,
    software: bool,
}

type GpuCell = Result<Mutex<GpuState>, String>;

fn gpu_cell() -> &'static GpuCell {
    static CELL: OnceLock<GpuCell> = OnceLock::new();
    CELL.get_or_init(|| {
        let gpu =
            drafftink_gpu::Gpu::headless_blocking(&drafftink_gpu::GpuOptions::default()).map_err(|e| e.to_string())?;
        let compositor =
            drafftink_gpu::Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).map_err(|e| e.to_string())?;
        let (summary, software) = (gpu.info().summary(), gpu.info().is_software());
        Ok(Mutex::new(GpuState { gpu, compositor, summary, software }))
    })
}

fn gpu_state() -> Result<MutexGuard<'static, GpuState>, String> {
    match gpu_cell() {
        Ok(m) => Ok(m.lock().unwrap_or_else(PoisonError::into_inner)),
        Err(e) => Err(e.clone()),
    }
}

/// What the machine offers for pixel probes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuStatus {
    /// An adapter works. `software` is true for llvmpipe, lavapipe and the like.
    Available {
        /// The adapter, one line.
        adapter: String,
        /// A CPU rasteriser, not a GPU.
        software: bool,
    },
    /// No adapter, and why.
    Missing(String),
}

/// Whether pixel probes can run here. Creates the device on first use.
pub fn gpu_status() -> GpuStatus {
    match gpu_cell() {
        Ok(m) => {
            let g = m.lock().unwrap_or_else(PoisonError::into_inner);
            GpuStatus::Available { adapter: g.summary.clone(), software: g.software }
        }
        Err(e) => GpuStatus::Missing(e.clone()),
    }
}

/// True when `DRAFFTINK_REQUIRE_GPU=1`: a missing adapter fails tests instead of skipping them.
pub fn gpu_required() -> bool {
    std::env::var("DRAFFTINK_REQUIRE_GPU").is_ok_and(|v| !v.is_empty() && v != "0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_harness_has_every_port_and_a_headless_one_has_none_of_the_optional_ones() {
        use drafftink_engine::ports::PortKind;
        let full = Harness::new();
        let bare = Harness::headless();
        for kind in [PortKind::Clipboard, PortKind::Storage, PortKind::Files, PortKind::Renderer, PortKind::Net] {
            assert!(full.session.has_port(kind), "{kind:?}");
            assert!(!bare.session.has_port(kind), "{kind:?}");
        }
    }

    #[test]
    fn fixture_paths_cannot_leave_the_fixture_directory() {
        let mut h = Harness::new();
        for bad in ["", "/etc/passwd", "../Cargo.toml", "legacy/../../Cargo.toml"] {
            assert_eq!(h.load_fixture(bad), Err(HarnessError::BadPath(bad.to_owned())), "{bad}");
        }
        let e = h.load_fixture("legacy/does_not_exist.json").unwrap_err();
        assert!(matches!(e, HarnessError::Unreadable { .. }), "{e}");
    }

    #[test]
    fn a_fixture_replaces_the_document_without_history_and_is_not_dirty() {
        let mut h = Harness::new();
        let out = h.load_fixture("legacy/welcome.json").unwrap();
        assert_eq!(out["elements"], json!(h.session.tab().doc.store().len()));
        assert!(h.session.tab().doc.store().len() > 40);
        assert!(!h.session.tab().dirty());
        assert!(!h.session.tab().doc.can_undo());
    }

    #[test]
    fn the_clock_moves_only_when_told() {
        let mut h = Harness::new();
        let t0 = h.advance_clock(0).unwrap()["now_ms"].as_u64().unwrap();
        let t1 = h.advance_clock(250).unwrap()["now_ms"].as_u64().unwrap();
        assert_eq!(t1, t0 + 250);
        assert_eq!(h.session.now_ms(), t1);
    }

    #[test]
    fn the_scene_probe_counts_kinds_groups_bindings_and_reads_one_element() {
        let mut h = Harness::new();
        h.load_generated(20, 1).unwrap();
        let p = h.scene_probe(Some("e4"), true).unwrap();
        assert_eq!(p["total"], json!(20));
        assert_eq!(p["kinds"]["arrow"], json!(2));
        assert_eq!(p["bound_arrows"], json!(2));
        assert_eq!(p["groups"], json!(2));
        assert_eq!(p["element"]["kind"], json!("arrow"));
        assert_eq!(p["element"]["binding"]["start"]["target"], json!("e0"));
        assert_eq!(p["element"]["binding"]["end"]["target"], json!("e1"));
        assert_eq!(p["element"]["points"].as_array().map(Vec::len), Some(2));
        assert!(p["outline_text"].as_str().unwrap().contains("e4"));
        assert_eq!(h.scene_probe(Some("nope"), false), Err(HarnessError::NoSuchElement("nope".into())));
    }

    #[test]
    fn a_pixel_probe_without_an_adapter_says_so_and_with_one_sees_ink() {
        let mut h = Harness::new();
        h.load_generated(10, 1).unwrap();
        let args = PixelArgs { size: [300, 200], points: vec![] };
        match gpu_status() {
            GpuStatus::Missing(_) => {
                assert!(!gpu_required(), "DRAFFTINK_REQUIRE_GPU is set but there is no adapter");
                assert!(matches!(h.pixel_probe(&args), Err(HarnessError::NoGpu(_))));
            }
            GpuStatus::Available { .. } => {
                let out = h.pixel_probe(&args).unwrap();
                assert!(out["hardware"] != out["software"]);
                assert!(out["ink_px"].as_u64().unwrap() > 100, "{out}");
            }
        }
    }
}
