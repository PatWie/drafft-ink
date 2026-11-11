//! The web platform (wasm32): browser storage, downloads and file inputs, the async clipboard, the
//! `WebSocket` transport. [`platform`] builds the [`Platform`] that `App::new` takes.
//!
//! What cannot be done in a browser, and what the app does instead:
//!
//! | Port | In the browser |
//! |---|---|
//! | `Storage` | IndexedDB behind an in-memory cache ([`WebStorage`]); read before the app starts |
//! | `Files` | a download for `FileRef::Download`, the `Documents` namespace for `FileRef::Stored`; no paths |
//! | `Net` | `WebTransport`; the server is the one that delivered the page (`/ws`) |
//! | `Renderer` | none: PNG export needs an asynchronous read-back (see `docs/web-app.md`) |
//! | `Clipboard` | the shared slot the shell fills from `paste` events |

mod assets;
mod cache;
mod dom;
mod files;
mod host;
mod idb;
mod net;
mod pure;
mod rng;
mod storage;

use drafftink_bridge::host::{JobSender, channel};
use drafftink_engine::Ports;
use drafftink_gpu::RedrawReason;
use drafftink_scene::RandomIds;

use crate::clock::AppClock;
use crate::config::StartupLink;
use drafftink_ui::theme::UiFont;

use crate::platform::{FontFile, Platform, SharedClipboard, WakeCell};

pub use storage::WebStorage;

use files::WebFiles;
use host::WebHost;
use net::WebNet;
use rng::CryptoRng;

/// The web platform could not be set up.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebError {
    /// There is no `window`: the code does not run in a page.
    #[error("this code must run in a browser page")]
    NoWindow,
    /// A browser call failed.
    #[error("the browser refused a request: {0}")]
    Browser(String),
}

/// What the page tells the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebOptions {
    /// The id of the `<canvas>` the shell draws into.
    pub canvas_id: String,
    /// The page address (`location.href`), for the room link in its query or fragment.
    pub page_url: String,
    /// Apple platform: `Mod` is Cmd.
    pub is_mac: bool,
}

impl WebOptions {
    /// Reads the page address and the platform from the browser.
    pub fn detect(canvas_id: impl Into<String>) -> Result<Self, WebError> {
        let window = dom::window()?;
        let location = window.location();
        let navigator = window.navigator();
        let page_url = location.href().map_err(|e| WebError::Browser(dom::js_error(&e)))?;
        let platform = navigator.platform().unwrap_or_default();
        let user_agent = navigator.user_agent().unwrap_or_default();
        Ok(Self { canvas_id: canvas_id.into(), page_url, is_mac: pure::is_mac_platform(&platform, &user_agent) })
    }

    /// The room to join, if the address names one.
    pub fn startup_link(&self) -> StartupLink {
        StartupLink::parse(&self.page_url)
    }
}

/// A [`Platform`] plus the handles the page keeps.
pub struct WebPlatform {
    /// For `App::new`.
    pub platform: Platform,
    /// Queues requests for the app: `window.drafftink.*` and the agent relay submit through it. The app drains
    /// the other end (`Platform::bridge`) once per frame.
    pub jobs: JobSender,
    /// The storage, shared with the ports.
    pub storage: WebStorage,
}

/// Builds the platform: opens the storage (this waits for IndexedDB), then the ports.
///
/// A storage that cannot persist is not an error; the user gets a notice instead.
pub async fn platform(opts: WebOptions) -> Result<WebPlatform, WebError> {
    let storage = WebStorage::open().await;
    if let Some(notice) = storage.notice() {
        dom::show_notice(&notice, 20);
    }
    storage.flush_on_page_hide();
    // The first frame needs the UI font. The page preloaded it, so this mostly waits for the module to compile.
    // Without it GelPen draws the text (`drafftink_ui::theme::install_fonts`).
    let ubuntu = FontFile::Ui(UiFont::UbuntuLight);
    match assets::fetch_font(ubuntu).await {
        Ok(bytes) => drafftink_ui::theme::provide_font(UiFont::UbuntuLight, bytes),
        Err(e) => log::warn!("could not load {}: {e}", ubuntu.file_name()),
    }

    let wake = WakeCell::new();
    let clipboard = SharedClipboard::new();
    let location = dom::window()?.location();
    let net = WebNet::new(wake.hook(RedrawReason::Collab), &location);
    let ports = Ports::new(Box::new(AppClock::new()), Box::new(RandomIds(CryptoRng)))
        .with_clipboard(Box::new(clipboard.clone()))
        .with_storage(Box::new(storage.clone()))
        .with_files(Box::new(WebFiles::new(storage.clone())))
        .with_net(Box::new(net));

    let (jobs, bridge) = channel(wake.hook(RedrawReason::Load), None);
    let platform = Platform {
        ports,
        host: Box::new(WebHost),
        wake,
        clipboard,
        typesetter: drafftink_engine::services::LazyTypesetter::shared(),
        bridge: Some(bridge),
        is_mac: opts.is_mac,
    };
    Ok(WebPlatform { platform, jobs, storage })
}
