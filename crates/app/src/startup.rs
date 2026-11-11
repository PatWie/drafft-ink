//! Start-up: what happens between building the session and the first frame.

use drafftink_engine::cmd::doc::use_stored_default_target;
use drafftink_engine::{Origin, Session};
use drafftink_scene::SceneRead;
use serde_json::{Value, json};

use crate::config::AppConfig;

/// Loads the settings and the saved workspace (or shows the intro document on a first run), opens the file
/// named on the command line and joins the room named by the link. Returns notes for the person, in plain
/// words, for what could not be done.
pub(crate) fn start(session: &mut Session, config: &AppConfig) -> Vec<String> {
    let mut notes = Vec::new();
    if config.stored_default_target {
        use_stored_default_target(session, true);
    }
    match session.load_settings() {
        Ok(load) => {
            if let Some(why) = &load.unreadable {
                notes.push(format!("The saved settings could not be read ({why}), so the defaults are in use."));
            }
            if let Some(n) = load.newer_schema {
                notes.push(format!("The saved settings come from a newer Drafft.ink (format {n}) and are left alone."));
            }
        }
        Err(r) => notes.push(r.message),
    }
    let mut restored = 0;
    let mut found = false;
    match session.restore_workspace() {
        Ok(r) => {
            restored = r.restored;
            found = r.found;
            if let Some(why) = &r.unreadable {
                notes.push(format!("The saved workspace could not be read ({why}). Starting with a blank document."));
            }
            if let Some(n) = r.newer_schema {
                notes
                    .push(format!("The saved workspace comes from a newer Drafft.ink (format {n}) and is left alone."));
            }
            if !r.skipped.is_empty() {
                notes.push(format!("{} saved tab(s) could not be opened.", r.skipped.len()));
            }
        }
        Err(r) => notes.push(r.message),
    }
    if let Some(file) = &config.open {
        let mode = if restored == 0 { "replace" } else { "new_tab" };
        run(session, &mut notes, "doc.open", json!({ "source": file, "mode": mode }));
    } else if !found && restored == 0 && config.join.link.room.is_none() {
        run(session, &mut notes, "doc.show_intro", json!({}));
    }
    if let Some(room) = &config.join.link.room {
        // A room fills an empty tab. Work the person already has stays in its own tab.
        if session.store().len() > 0 {
            run(session, &mut notes, "tab.new", json!({}));
        }
        let server = config.join.link.server.clone().or_else(|| config.default_server.clone());
        run(session, &mut notes, "collab.connect", json!({ "url": server, "room": room, "token": config.token_arg() }));
    }
    notes
}

/// Runs a command for the person; a refusal becomes a note.
fn run(session: &mut Session, notes: &mut Vec<String>, id: &str, args: Value) {
    if let Err(r) = session.execute_from(Origin::Ui, id, args) {
        notes.push(r.message);
    }
}
