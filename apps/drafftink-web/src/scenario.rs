//! `window.drafftink.runScenario(key)` and `scenarioReport()`: the page side of the scenario driver
//! (`docs/performance.md` section 5.3, driver `scripts/perf_scenarios.py`).
//!
//! `runScenario('pan@10k')` turns recording on, puts the generated scene on screen, waits until the app has drawn it
//! and gone quiet, resets the profiler and resolves. The driver then sends real browser input, and
//! `scenarioReport()` returns the `ScenarioReport` of the window that followed. Builds without the `scenarios`
//! feature reject `runScenario`: the generated scenes are not part of the shipped app.

use std::cell::RefCell;

use drafftink_profile::ScenarioKey;
use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;

use crate::js;

#[cfg(feature = "scenarios")]
/// How long the app must draw nothing before the window opens, in ms.
const QUIET_MS: f64 = 1_000.0;
#[cfg(feature = "scenarios")]
/// The longest wait for a scene and for quiet, in ms. A 100,000-element scene takes several seconds.
const LIMIT_MS: f64 = 180_000.0;

thread_local! {
    /// The scenario `runScenario` last started, for the report.
    static CURRENT: RefCell<Option<ScenarioKey>> = const { RefCell::new(None) };
}

#[cfg(feature = "scenarios")]
fn now_ms() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
}

#[cfg(feature = "scenarios")]
async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[cfg(feature = "scenarios")]
/// Waits until no frame was drawn for [`QUIET_MS`].
async fn wait_quiet() {
    let start = now_ms();
    let (mut frames, mut since) = (drafftink_gpu::frames_drawn(), now_ms());
    while now_ms() - start < LIMIT_MS {
        sleep_ms(50).await;
        let now = drafftink_gpu::frames_drawn();
        if now != frames {
            (frames, since) = (now, now_ms());
        } else if now_ms() - since >= QUIET_MS {
            return;
        }
    }
}

#[cfg(feature = "scenarios")]
async fn run(text: String, loader: drafftink_app::SceneLoader) -> Result<JsValue, JsValue> {
    let key: ScenarioKey = text.parse().map_err(|e: drafftink_profile::KeyError| JsValue::from_str(&e.to_string()))?;
    drafftink_profile::set_enabled(true);
    let t0 = now_ms();
    let mut scene_ms = None;
    if let Some(install) =
        drafftink_app::scene_for(&key).map_err(|e| JsValue::from_str(&format!("the scene could not be generated: {e}")))?
    {
        let (before, frames) = (loader.applied(), drafftink_gpu::frames_drawn());
        loader.load(install);
        while loader.applied() == before {
            if now_ms() - t0 > LIMIT_MS {
                return Err(JsValue::from_str("the app did not install the scene in time"));
            }
            sleep_ms(5).await;
        }
        while drafftink_gpu::frames_drawn() == frames && now_ms() - t0 < LIMIT_MS {
            sleep_ms(2).await;
        }
        scene_ms = Some(now_ms() - t0);
    }
    wait_quiet().await;
    drafftink_profile::reset();
    CURRENT.with(|c| *c.borrow_mut() = Some(key));
    let out = Object::new();
    let _ = Reflect::set(&out, &"key".into(), &JsValue::from_str(&key.to_string()));
    let _ = Reflect::set(&out, &"install_ms".into(), &JsValue::from_f64(scene_ms.unwrap_or(0.0)));
    Ok(out.into())
}

/// Adds `runScenario` and `scenarioReport` to `window.drafftink`.
#[cfg(feature = "scenarios")]
pub(crate) fn install(loader: drafftink_app::SceneLoader) -> Result<(), JsValue> {
    let api = api_object()?;
    let run = Closure::<dyn Fn(JsValue) -> JsValue>::new(move |key: JsValue| {
        let loader = loader.clone();
        let text = key.as_string().unwrap_or_default();
        wasm_bindgen_futures::future_to_promise(run(text, loader)).into()
    });
    Reflect::set(&api, &"runScenario".into(), run.as_ref())?;
    run.forget();
    install_report(&api)
}

/// Without the `scenarios` feature: `runScenario` rejects with the way to get it.
#[cfg(not(feature = "scenarios"))]
pub(crate) fn install_unavailable() -> Result<(), JsValue> {
    let api = api_object()?;
    let run = Closure::<dyn Fn(JsValue) -> JsValue>::new(|_: JsValue| {
        js_sys::Promise::reject(&JsValue::from_str(
            "this build has no scenarios; build the web app with `scripts/build-web.sh --features scenarios`",
        ))
        .into()
    });
    Reflect::set(&api, &"runScenario".into(), run.as_ref())?;
    run.forget();
    install_report(&api)
}

fn api_object() -> Result<Object, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    Reflect::get(&window, &"drafftink".into())?.dyn_into::<Object>()
}

fn install_report(api: &Object) -> Result<(), JsValue> {
    let report = Closure::<dyn Fn() -> JsValue>::new(|| {
        let key = CURRENT.with(|c| *c.borrow());
        let Some(key) = key else { return JsValue::NULL };
        let mut report = key.report("web", drafftink_profile::snapshot());
        report.meta.insert("build".into(), "release".into());
        match serde_json::to_value(&report) {
            Ok(v) => js::from_json(&v),
            Err(_) => JsValue::NULL,
        }
    });
    Reflect::set(api, &"scenarioReport".into(), report.as_ref())?;
    report.forget();
    Ok(())
}
