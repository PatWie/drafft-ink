//! Rendering one shot: the real app, driven by commands, drawn offscreen.

use std::path::Path;

use base64::Engine as _;
use drafftink_app::native::{self, NativeOptions};
use drafftink_app::{App, AppConfig};
use drafftink_engine::{Origin, Refusal};
use drafftink_gpu::GpuError;
use drafftink_gpu::headless::{Headless, HeadlessOptions};
use serde_json::{Value, json};

use crate::png_out::{self, PngError};
use crate::spec::{Output, Shot, Step, Theme};

/// Frames run before the commands of a shot, so the app has seen its window size and shown the intro.
const WARM_UP_FRAMES: u32 = 3;
/// Frames run after the commands, so the camera, the ribbon and the fades have settled.
const SETTLE_FRAMES: u32 = 10;
/// The range of `scale` that `doc.export` accepts.
const EXPORT_SCALE: std::ops::RangeInclusive<f64> = 0.25..=4.0;

/// Why a shot could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("shot `{shot}`: step `{step}` was refused: {refusal}")]
    Step { shot: String, step: String, refusal: String },
    #[error("the GPU: {0}")]
    Gpu(#[from] GpuError),
    #[error(transparent)]
    Png(#[from] PngError),
    #[error("a scratch folder for the app: {0}")]
    Scratch(#[from] std::io::Error),
    #[error("the export answered without picture bytes")]
    NoBytes,
    #[error("the export bytes are not base64: {0}")]
    Base64(#[from] base64::DecodeError),
}

fn refusal_text(r: &Refusal) -> String {
    format!("{} ({})", r.message, r.code)
}

/// A real app over the desktop platform, with a scratch data folder so that no setting or workspace of the person
/// who runs the tool is read, and the first-run intro document is shown.
fn app(data: &Path) -> App {
    let platform = native::platform(&NativeOptions { data_dir: Some(data.to_owned()) });
    App::new(platform, AppConfig::default())
}

fn execute(app: &mut App, shot: &str, id: &str, args: Value) -> Result<Value, RenderError> {
    app.session_mut().execute_from(Origin::Script, id, args).map_err(|r| RenderError::Step {
        shot: shot.to_owned(),
        step: id.to_owned(),
        refusal: refusal_text(&r),
    })
}

fn run_steps(app: &mut App, shot: &Shot) -> Result<(), RenderError> {
    for Step { id, args } in &shot.steps {
        let args = if args.is_null() { json!({}) } else { args.clone() };
        execute(app, &shot.name, id, args)?;
    }
    Ok(())
}

/// Renders `shot` and returns the PNG.
pub fn render(shot: &Shot) -> Result<Vec<u8>, RenderError> {
    // The folder outlives the app: dropping an app writes its workspace.
    let data = tempfile::tempdir()?;
    match shot.output {
        Output::Window { size, scale, theme, touch } => window(shot, data.path(), size, scale, theme, touch),
        Output::Export { width, padding, theme } => export(shot, data.path(), width, padding, theme),
    }
}

fn window(
    shot: &Shot,
    data: &Path,
    size: [f32; 2],
    scale: f32,
    theme: Theme,
    touch: bool,
) -> Result<Vec<u8>, RenderError> {
    let options = HeadlessOptions { size_points: size, scale, touch_first: touch };
    let mut shell = Headless::new(app(data), options)?;
    shell.settle(WARM_UP_FRAMES);
    execute(shell.app_mut(), &shot.name, "settings.set", json!({ "key": "theme", "value": theme.setting() }))?;
    run_steps(shell.app_mut(), shot)?;
    shell.settle(SETTLE_FRAMES);
    let image = shell.screenshot()?;
    Ok(png_out::encode_opaque(image.width, image.height, &image.rgba)?)
}

/// The scale that makes the export `width` pixels wide for a document `content` world units wide.
fn export_scale(width: u32, content: f64, padding: f64) -> f64 {
    (f64::from(width) / (content + 2.0 * padding).max(1.0)).clamp(*EXPORT_SCALE.start(), *EXPORT_SCALE.end())
}

fn export(shot: &Shot, data: &Path, width: u32, padding: f64, theme: Theme) -> Result<Vec<u8>, RenderError> {
    let mut app = app(data);
    run_steps(&mut app, shot)?;
    let content = execute(&mut app, &shot.name, "doc.inspect", json!({}))?
        .pointer("/bounds/w")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let out = execute(
        &mut app,
        &shot.name,
        "doc.export",
        json!({
            "format": "png",
            "scope": "all",
            "scale": export_scale(width, content, padding),
            "padding": padding,
            "background": true,
            "dark": theme == Theme::Dark,
            "embed_scene": false,
        }),
    )?;
    let text = out.pointer("/bytes/base64").and_then(Value::as_str).ok_or(RenderError::NoBytes)?;
    let png = base64::engine::general_purpose::STANDARD.decode(text)?;
    Ok(png_out::recompress(&png)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_scale_reaches_the_width_and_stays_in_range() {
        assert!((export_scale(1600, 1360.0, 20.0) - 1.142_857).abs() < 1e-3);
        assert_eq!(export_scale(1600, 10.0, 0.0), 4.0);
        assert_eq!(export_scale(100, 100_000.0, 0.0), 0.25);
    }
}
