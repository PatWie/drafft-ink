//! The collaboration dialog and the presence panel.
//!
//! The dialog reads the link state from the model (`model.collab`, fed by the app) and the server URL,
//! name and colour from the settings. Every button is a `collab.*` command; one that the build does not
//! register, or that refuses now, is greyed with the refusal as its tooltip. The pure functions that
//! build the calls are `pub(crate)` and tested without drawing.

use drafftink_engine::Origin;
use drafftink_scene::Color;
use egui::{Align2, Area, Color32, Context, Id, Modal, Order, RichText, Ui, vec2};
use serde_json::{Value, json};

use crate::action::Call;
use crate::model::{CollabStatus, CollabView};
use crate::state::UiState;
use crate::surfaces::dialogs::{Close, enter_pressed, show_modal, text_field};
use crate::theme::{self, MARGIN};
use crate::widgets::{Ctl, Cx, panel_frame, section_label, separator};

/// The server the settings default to, shown when the setting is missing.
const DEFAULT_SERVER: &str = "ws://localhost:3030/ws";
/// The colour choices of the cursor, with the names the controls carry.
pub(crate) const USER_COLORS: [(&str, &str); 9] = [
    ("#ef4444", "Red"),
    ("#f97316", "Orange"),
    ("#eab308", "Yellow"),
    ("#22c55e", "Green"),
    ("#06b6d4", "Cyan"),
    ("#3b82f6", "Blue"),
    ("#6366f1", "Indigo"),
    ("#a855f7", "Purple"),
    ("#ec4899", "Pink"),
];
/// Peers listed in the presence panel; the rest is "+N more".
pub(crate) const MAX_PEERS_SHOWN: usize = 8;
/// Longest display name (the engine's limit).
const MAX_NAME_CHARS: usize = 40;

// ---- the calls ----

fn call(id: &str, args: Value) -> Call {
    Call::new(Origin::Ui, id, args)
}

/// `collab.connect`. An empty field leaves the URL out, so the engine uses the setting.
pub(crate) fn connect_call(url: &str) -> Call {
    let url = url.trim();
    call("collab.connect", if url.is_empty() { json!({}) } else { json!({ "url": url }) })
}

/// `collab.disconnect`.
pub(crate) fn disconnect_call() -> Call {
    call("collab.disconnect", json!({}))
}

/// `collab.join_room`. An empty room name leaves it out: the engine then creates a random room.
pub(crate) fn join_call(room: &str) -> Call {
    let room = room.trim();
    call("collab.join_room", if room.is_empty() { json!({}) } else { json!({ "room": room }) })
}

/// `collab.leave_room`.
pub(crate) fn leave_call() -> Call {
    call("collab.leave_room", json!({}))
}

/// `collab.set_user` with the display name.
pub(crate) fn set_user_name_call(name: &str) -> Call {
    call("collab.set_user", json!({ "name": name.trim() }))
}

/// `collab.set_user` with the cursor colour.
pub(crate) fn set_user_color_call(color: &str) -> Call {
    call("collab.set_user", json!({ "color": color }))
}

/// The name to send when the field is left or Enter is pressed: only when it differs from the last
/// value sent (or stored).
pub(crate) fn name_to_send(text: &str, last: &str) -> Option<String> {
    let t = text.trim();
    (t != last.trim()).then(|| t.to_owned())
}

// ---- status ----

/// How the status dot is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Connected.
    Ok,
    /// Connecting.
    Warn,
    /// Not connected.
    Off,
    /// Failed.
    Bad,
}

/// The status line and the colour of its dot.
pub(crate) fn status_line(c: &CollabView) -> (Tone, String) {
    match (&c.status, &c.room) {
        (CollabStatus::Connected, Some(room)) => (Tone::Ok, format!("Connected to room: {room}")),
        (CollabStatus::Connected, None) => (Tone::Ok, "Connected (no room)".to_owned()),
        (CollabStatus::Connecting, _) => (Tone::Warn, "Connecting...".to_owned()),
        (CollabStatus::Disconnected, _) => (Tone::Off, "Not connected".to_owned()),
        (CollabStatus::Failed(reason), _) if reason.is_empty() => (Tone::Bad, "Connection error".to_owned()),
        (CollabStatus::Failed(reason), _) => (Tone::Bad, format!("Connection error: {reason}")),
    }
}

/// "1 person online" and so on, when in a room.
pub(crate) fn people_line(c: &CollabView) -> Option<String> {
    c.room.as_ref()?;
    Some(match c.peers.len() {
        1 => "1 person online".to_owned(),
        n => format!("{n} people online"),
    })
}

/// The peers the panel lists and how many more there are.
pub(crate) fn presence_rows(c: &CollabView) -> Option<(&[crate::model::Peer], usize)> {
    if c.room.is_none() || c.peers.is_empty() {
        return None;
    }
    let shown = c.peers.len().min(MAX_PEERS_SHOWN);
    Some((&c.peers[..shown], c.peers.len() - shown))
}

// ---- the dialog ----

/// What the text fields hold while the dialog is open.
#[derive(Clone, Default)]
struct Draft {
    loaded: bool,
    url: String,
    room: String,
    name: String,
    /// The last name that was sent or stored.
    name_sent: String,
}

fn tone_color(pal: &theme::Palette, tone: Tone) -> Color32 {
    match tone {
        Tone::Ok => pal.ok,
        Tone::Warn => pal.warn,
        Tone::Off => pal.muted,
        Tone::Bad => pal.danger,
    }
}

fn dot(ui: &mut Ui, color: Color32, diameter: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(diameter, diameter), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), diameter / 2.0, color);
}

/// Draws the dialog.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let key = Id::unique("collab_draft");
    if !st.collab_open {
        ctx.data_mut(|d| d.remove_temp::<Draft>(key));
        return;
    }
    let mut draft: Draft = ctx.data_mut(|d| d.get_temp(key)).unwrap_or_default();
    if !draft.loaded {
        draft.loaded = true;
        draft.url = cx.model.setting_str("collab.server_url").unwrap_or(DEFAULT_SERVER).to_owned();
        draft.name = cx.model.setting_str("collab.user_name").unwrap_or_default().to_owned();
        draft.name_sent = draft.name.clone();
    }
    show_modal(ctx, cx, Modal::new(Id::unique("collab_dialog")), "Collaborate", 320.0, &Close::Overlays, |ui, cx| {
        body(ui, cx, &mut draft);
    });
    ctx.data_mut(|d| d.insert_temp(key, draft));
}

fn body(ui: &mut Ui, cx: &mut Cx<'_>, draft: &mut Draft) {
    let pal = cx.pal;
    let collab = cx.model.collab.clone();
    let linked = matches!(collab.status, CollabStatus::Connecting | CollabStatus::Connected);

    // Status.
    let (tone, text) = status_line(&collab);
    ui.horizontal(|ui| {
        dot(ui, tone_color(&pal, tone), 10.0);
        ui.label(RichText::new(text).size(13.0).color(pal.text));
    });
    if let Some(people) = people_line(&collab) {
        ui.label(RichText::new(people).size(12.0).color(pal.muted));
    }
    ui.add_space(10.0);

    // Server.
    section_label(ui, &pal, "Server URL");
    let reason =
        if linked { Some("Disconnect first to change the server.".to_owned()) } else { cx.refusal("collab.connect") };
    text_field(ui, &mut draft.url, DEFAULT_SERVER, reason, None, "Server URL");
    ui.add_space(6.0);
    if linked {
        let c = disconnect_call();
        cx.text_button(ui, &Ctl::new(&c.id).args(c.args.clone()), "Disconnect", false);
    } else {
        let c = connect_call(&draft.url);
        cx.text_button(ui, &Ctl::new(&c.id).args(c.args.clone()), "Connect", true);
    }

    // Room.
    if collab.status == CollabStatus::Connected {
        ui.add_space(10.0);
        separator(ui, &pal);
        ui.add_space(10.0);
        section_label(ui, &pal, "Room");
        if let Some(room) = &collab.room {
            draft.room.clone_from(room);
            text_field(ui, &mut draft.room, "", Some("Leave the room to join another.".to_owned()), None, "Room");
            ui.add_space(6.0);
            let c = leave_call();
            cx.text_button(ui, &Ctl::new(&c.id).args(c.args.clone()), "Leave room", false);
        } else {
            let reason = cx.refusal("collab.join_room");
            text_field(ui, &mut draft.room, "Room name (empty makes a new room)", reason, None, "Room");
            ui.add_space(6.0);
            let c = join_call(&draft.room);
            let text = if draft.room.trim().is_empty() { "Create room" } else { "Join room" };
            cx.text_button(ui, &Ctl::new(&c.id).args(c.args.clone()), text, true);
        }
    }

    // Profile.
    ui.add_space(10.0);
    separator(ui, &pal);
    ui.add_space(10.0);
    section_label(ui, &pal, "Your profile");
    section_label(ui, &pal, "Display name");
    let reason = cx.refusal("collab.set_user");
    let resp = text_field(ui, &mut draft.name, "Anonymous", reason, Some(MAX_NAME_CHARS), "Display name");
    let left = resp.lost_focus() || (resp.has_focus() && enter_pressed(ui));
    if let Some(name) = name_to_send(&draft.name, &draft.name_sent).filter(|_| left) {
        let c = set_user_name_call(&name);
        cx.run(&c.id, c.args);
        draft.name_sent = name;
    }
    ui.add_space(6.0);
    section_label(ui, &pal, "Cursor colour");
    let mine = cx.model.setting_str("collab.user_color").unwrap_or("#6366f1").to_owned();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
        for (hex, name) in USER_COLORS {
            let c = set_user_color_call(hex);
            let ctl = Ctl::new(&c.id).args(c.args.clone()).label(name).selected(mine.eq_ignore_ascii_case(hex));
            cx.swatch(ui, &ctl, Color::parse(hex).ok().map(to32), 20.0);
        }
    });
}

fn to32(c: Color) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

// ---- presence ----

/// Draws the presence panel: bottom right, who else is in the room.
pub fn show_presence(ctx: &Context, cx: &mut Cx<'_>, _st: &mut UiState) {
    let Some((peers, more)) = presence_rows(&cx.model.collab) else {
        return;
    };
    let pal = cx.pal;
    Area::new(Id::unique("presence_panel"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-MARGIN, -MARGIN))
        .order(Order::Middle)
        .show(ctx, |ui| {
            panel_frame(&pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
                for p in peers {
                    ui.horizontal(|ui| {
                        let color = Color::parse(&p.color).ok().map_or(pal.muted, to32);
                        dot(ui, color, 8.0);
                        let name = if p.name.trim().is_empty() { "Anonymous" } else { p.name.trim() };
                        ui.label(RichText::new(name).size(11.0).color(pal.text));
                    });
                }
                if more > 0 {
                    ui.label(RichText::new(format!("+{more} more")).size(11.0).color(pal.muted));
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Peer;

    fn peer(name: &str) -> Peer {
        Peer { name: name.to_owned(), color: "#ef4444".to_owned(), has_cursor: false }
    }

    #[test]
    fn calls_have_the_documented_arguments() {
        assert_eq!(connect_call(" wss://h/ws ").args, json!({"url": "wss://h/ws"}));
        assert_eq!(connect_call("  ").args, json!({}));
        assert_eq!(connect_call("x").id, "collab.connect");
        assert_eq!(disconnect_call().id, "collab.disconnect");
        assert_eq!(join_call(" abc_1 ").args, json!({"room": "abc_1"}));
        assert_eq!(join_call("").args, json!({}));
        assert_eq!(leave_call().id, "collab.leave_room");
        assert_eq!(set_user_name_call(" Ada ").args, json!({"name": "Ada"}));
        assert_eq!(set_user_color_call("#3b82f6").args, json!({"color": "#3b82f6"}));
        for c in [connect_call("x"), join_call("r"), leave_call(), set_user_name_call("n")] {
            assert_eq!(c.origin, Origin::Ui);
        }
    }

    #[test]
    fn a_name_is_sent_only_when_it_changed() {
        assert_eq!(name_to_send("Ada", "Ada"), None);
        assert_eq!(name_to_send(" Ada ", "Ada"), None);
        assert_eq!(name_to_send("Bob", "Ada"), Some("Bob".to_owned()));
        assert_eq!(name_to_send("", "Ada"), Some(String::new()), "clearing the name is a change");
        assert_eq!(name_to_send("", ""), None);
    }

    #[test]
    fn status_lines() {
        let mut c = CollabView::default();
        assert_eq!(status_line(&c), (Tone::Off, "Not connected".to_owned()));
        c.status = CollabStatus::Connecting;
        assert_eq!(status_line(&c), (Tone::Warn, "Connecting...".to_owned()));
        c.status = CollabStatus::Connected;
        assert_eq!(status_line(&c), (Tone::Ok, "Connected (no room)".to_owned()));
        assert_eq!(people_line(&c), None);
        c.room = Some("abc".to_owned());
        c.peers = vec![peer("a")];
        assert_eq!(status_line(&c), (Tone::Ok, "Connected to room: abc".to_owned()));
        assert_eq!(people_line(&c).as_deref(), Some("1 person online"));
        c.peers.push(peer("b"));
        assert_eq!(people_line(&c).as_deref(), Some("2 people online"));
        c.status = CollabStatus::Failed("timed out".to_owned());
        assert_eq!(status_line(&c), (Tone::Bad, "Connection error: timed out".to_owned()));
        c.status = CollabStatus::Failed(String::new());
        assert_eq!(status_line(&c).1, "Connection error");
    }

    #[test]
    fn presence_lists_eight_and_counts_the_rest() {
        let mut c = CollabView::default();
        assert!(presence_rows(&c).is_none(), "no room, no panel");
        c.room = Some("r".to_owned());
        assert!(presence_rows(&c).is_none(), "no peers, no panel");
        c.peers = (0..11).map(|i| peer(&format!("p{i}"))).collect();
        let (shown, more) = presence_rows(&c).unwrap();
        assert_eq!((shown.len(), more), (8, 3));
        c.peers.truncate(8);
        assert_eq!(presence_rows(&c).unwrap().1, 0);
    }

    #[test]
    fn the_nine_colours_are_valid_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for (hex, _) in USER_COLORS {
            assert!(Color::parse(hex).is_ok(), "{hex}");
            assert!(seen.insert(hex));
        }
        assert_eq!(seen.len(), 9);
    }
}
