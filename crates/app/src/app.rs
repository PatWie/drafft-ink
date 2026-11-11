//! [`App`]: the `ShellApp` of Drafft.ink.

use drafftink_gpu::{FrameCx, FrameOutput, SetupCx, ShellApp};

use crate::config::AppConfig;
use crate::platform::Platform;

/// The application. Build it with [`App::new`] and give it to `drafftink_gpu::run`.
pub struct App {
    platform: Platform,
    config: AppConfig,
}

impl App {
    /// An app over a platform.
    pub fn new(platform: Platform, config: AppConfig) -> Self {
        Self { platform, config }
    }
}

impl ShellApp for App {
    fn setup(&mut self, cx: &mut SetupCx<'_>) {
        self.platform.wake.set(cx.waker.clone());
        let _ = &self.config;
    }

    fn frame(&mut self, _ui: &mut egui::Ui, _cx: &mut FrameCx<'_>) -> FrameOutput {
        FrameOutput::default()
    }
}
