//! The About dialog (`ui.open_about`): the version and commit, the licence, and the offer of the source code.
//!
//! The app is AGPL-3.0, whose section 13 requires that users who reach a modified copy over a network can get its
//! corresponding source. This dialog is that offer for the web app and the native app. The address comes from
//! [`AboutInfo::source_url`](crate::model::AboutInfo): the project repository unless the operator of a hosted copy
//! (`drafftink-headless serve --source-url`) or the user of a native build (`DRAFFTINK_SOURCE_URL`) points it elsewhere.

use egui::{Context, Id, Label, Modal, RichText, Ui};

use crate::action::{HostRequest, UiAction};
use crate::state::UiState;
use crate::surfaces::dialogs::{Close, show_modal};
use crate::theme::{Palette, TextSize};
use crate::widgets::Cx;

/// Width of the dialog.
const WIDTH: f32 = 440.0;

/// The words about the bundled fonts. They are separate works under their own licences, listed in `ATTRIBUTION.md`.
pub const FONTS_NOTE: &str = "The fonts that come with the app are separate works with their own licences, not covered by the \
AGPL. ATTRIBUTION.md in the source code lists each one.";

/// Draws the dialog when `st.about_open` is set.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !st.about_open {
        return;
    }
    show_modal(ctx, cx, Modal::new(Id::unique("about_dialog")), "About Drafft.ink", WIDTH, &Close::Overlays, body);
}

fn body(ui: &mut Ui, cx: &mut Cx<'_>) {
    let pal = cx.pal;
    let about = cx.model.about.clone();
    ui.label(RichText::new(format!("Version {} ({})", about.version, about.commit)).color(pal.text));
    ui.add_space(6.0);
    ui.label(format!("Licence: {}", about.license));
    ui.label("You may use, change and host Drafft.ink. If you run a changed copy for other people, you must offer them its source code.");
    ui.add_space(10.0);
    source(ui, cx, &pal, &about.source_url);
    ui.add_space(10.0);
    ui.add(Label::new(RichText::new(FONTS_NOTE).size(TextSize::Small.points()).color(pal.muted)));
}

/// The link to the source, and the address in words so that it can be read, copied and checked.
fn source(ui: &mut Ui, cx: &mut Cx<'_>, pal: &Palette, url: &str) {
    if ui.link("Source code").clicked() {
        cx.emit(UiAction::Host(HostRequest::OpenUrl(url.to_owned())));
    }
    ui.add(Label::new(RichText::new(url).size(TextSize::Small.points()).color(pal.muted)).selectable(true));
}
