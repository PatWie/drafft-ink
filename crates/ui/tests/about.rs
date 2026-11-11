//! The About dialog (`ui.open_about`): version, commit, licence and the offer of the source code that AGPL-3.0
//! section 13 asks for. The address is the repository from the package metadata unless the shell sets another.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod dialog;

use common::Fixture;
use drafftink_ui::model::AboutInfo;
use drafftink_ui::surfaces::about::FONTS_NOTE;
use drafftink_ui::{HostRequest, Verb};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

type H = Harness<'static, Fixture>;

fn open_with(about: Option<AboutInfo>) -> H {
    let mut f = dialog::fixture();
    if let Some(about) = about {
        f.model.about = about;
    }
    f.state.apply_verb(Verb::OpenAbout, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn the_dialog_names_version_commit_licence_and_the_project_repository_by_default() {
    let h = open_with(None);
    h.get_by_label("About Drafft.ink");
    h.get_by_label(&format!("Version {} (unknown)", env!("CARGO_PKG_VERSION")));
    h.get_by_label("Licence: AGPL-3.0-only");
    h.get_by_label("Source code");
    h.get_by_label(env!("CARGO_PKG_REPOSITORY"));
    assert!(env!("CARGO_PKG_REPOSITORY").starts_with("https://"), "the package metadata names the repository");
}

#[test]
fn the_source_link_asks_the_host_to_open_the_address() {
    let mut h = open_with(None);
    h.get_by_label("Source code").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.host, vec![HostRequest::OpenUrl(env!("CARGO_PKG_REPOSITORY").to_owned())]);
}

#[test]
fn an_operator_address_and_the_build_commit_replace_the_defaults() {
    let about = AboutInfo {
        commit: "abc1234".to_owned(),
        source_url: "https://git.example.org/team/drafftink-fork".to_owned(),
        ..AboutInfo::default()
    };
    let mut h = open_with(Some(about));
    h.get_by_label(&format!("Version {} (abc1234)", env!("CARGO_PKG_VERSION")));
    h.get_by_label("https://git.example.org/team/drafftink-fork");
    assert!(h.query_by_label(env!("CARGO_PKG_REPOSITORY")).is_none(), "the default address is gone");
    h.get_by_label("Source code").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.host, vec![HostRequest::OpenUrl("https://git.example.org/team/drafftink-fork".to_owned())]);
}

#[test]
fn the_fonts_are_said_to_have_their_own_licences_and_attribution_md_is_named() {
    let h = open_with(None);
    assert!(FONTS_NOTE.contains("ATTRIBUTION.md") && FONTS_NOTE.contains("licences"));
    h.get_by_label(FONTS_NOTE);
}

#[test]
fn the_main_menu_has_an_entry_that_opens_it_and_close_overlays_closes_it() {
    let mut f = dialog::fixture();
    f.state.apply_verb(Verb::ToggleMenu, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    // The menu is taller than the window, so it scrolls: bring the last row into view first.
    h.get_by_label("About Drafft.ink…").scroll_to_me();
    h.run();
    h.get_by_label("About Drafft.ink…").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::OpenAbout]);
    h.state_mut().dispatch();
    assert!(h.state().state.about_open && h.state().state.modal_open());
    h.state_mut().state.close_overlays();
    assert!(!h.state().state.about_open);
}
