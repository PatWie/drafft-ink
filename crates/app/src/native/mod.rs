//! The desktop platform: folders for settings and documents, files by path, the system file dialogs, the
//! WebSocket transport, and an offscreen renderer for PNG export and screenshots.
//!
//! The system clipboard is not here: the shell reads and writes it inside the event that needs it (see
//! `SharedClipboard`).

mod dirs;
mod files;
mod host;
mod net;
mod render;
mod rng;
mod storage;

use std::path::PathBuf;

use drafftink_engine::Ports;
use drafftink_engine::services::LazyTypesetter;
use drafftink_gpu::RedrawReason;
use drafftink_scene::RandomIds;

use crate::clock::AppClock;
use crate::platform::{Platform, SharedClipboard, WakeCell};

pub use dirs::{bridge_dir, data_dir};
pub use storage::NativeStore;

/// How the desktop platform is set up.
#[derive(Debug, Clone, Default)]
pub struct NativeOptions {
    /// Where settings, documents and the workspace live. `None`: the user's data folder (`data_dir`).
    pub data_dir: Option<PathBuf>,
}

/// Builds the desktop platform. The bridge is not part of it: the launcher starts a `BridgeServer` with
/// `platform.wake.hook(..)` and stores the host in `platform.bridge`.
pub fn platform(options: &NativeOptions) -> Platform {
    let root = options.data_dir.clone().or_else(data_dir).unwrap_or_else(|| PathBuf::from("drafftink-data"));
    let wake = WakeCell::new();
    let clipboard = SharedClipboard::new();
    let typesetter = LazyTypesetter::shared();
    let store = NativeStore::new(root.clone());
    let ports = Ports::new(Box::new(AppClock::new()), Box::new(RandomIds(rng::OsRng)))
        .with_clipboard(Box::new(clipboard.clone()))
        .with_storage(Box::new(store.clone()))
        .with_files(Box::new(files::NativeFiles::new(root.join("downloads"))))
        .with_renderer(Box::new(render::OffscreenRenderer::new(typesetter.clone())))
        .with_net(Box::new(net::NativeNet::new(wake.hook(RedrawReason::Collab))));
    Platform {
        ports,
        host: Box::new(host::NativeHost),
        wake,
        clipboard,
        typesetter,
        bridge: None,
        is_mac: cfg!(target_os = "macos"),
    }
}
