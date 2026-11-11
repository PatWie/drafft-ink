//! Colours, sizes, fonts and the egui visuals. The look of the old app, in one place.
//!
//! The old app hard-coded the accent `rgb(59,130,246)` in more than fifteen places and had no dark
//! theme. Here every colour comes from [`Palette`], chosen by the egui theme, which follows the
//! `theme` setting (`light`, `dark`, `system`).

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Theme, ThemePreference,
};

/// Distance of floating panels from the window edge.
pub const MARGIN: f32 = 12.0;
/// Corner radius of panels.
pub const PANEL_RADIUS: u8 = 8;
/// Corner radius of buttons.
pub const BUTTON_RADIUS: u8 = 6;
/// Corner radius of dialogs.
pub const DIALOG_RADIUS: u8 = 12;
/// Side of a tool button.
pub const TOOL_BUTTON: f32 = 34.0;
/// Side of a small icon button.
pub const SMALL_BUTTON: f32 = 28.0;
/// Width of the right panel.
pub const RIGHT_PANEL_WIDTH: f32 = 200.0;

/// The name of the egui font family used for headings and labels (GelPen).
pub const HAND_FONT: &str = "gelpen";
/// Previews of the canvas font families in the font picker.
pub const PREVIEW_SERIF: &str = "gelpen_serif";
/// Preview of Vanilla Extract.
pub const PREVIEW_VANILLA: &str = "vanilla_extract";

/// A font file the egui fonts need that the web build does not carry (it has no `default_fonts`: they are
/// 1 MB of the binary). The host loads each file and gives it to [`provide_font`]. On the desktop egui has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UiFont {
    /// Ubuntu Light, the proportional font. Needed for the first frame, so the web start awaits it.
    UbuntuLight,
    /// Hack, the monospace font; also the arrows and the shift sign of the keyboard shortcuts.
    Hack,
    /// The emoji-icon-font: the command key sign and a few symbols.
    EmojiIcons,
}

impl UiFont {
    /// Every UI font.
    pub const ALL: [UiFont; 3] = [UiFont::UbuntuLight, UiFont::Hack, UiFont::EmojiIcons];

    /// The name of the file the web build serves.
    pub fn file_name(self) -> &'static str {
        match self {
            UiFont::UbuntuLight => "Ubuntu-Light.ttf",
            UiFont::Hack => "Hack-Regular.ttf",
            UiFont::EmojiIcons => "emoji-icon-font.ttf",
        }
    }

    fn key(self) -> &'static str {
        match self {
            UiFont::UbuntuLight => "Ubuntu-Light",
            UiFont::Hack => "Hack",
            UiFont::EmojiIcons => "emoji-icon-font",
        }
    }
}

/// True when egui carries its own fonts in this build (the desktop), so no [`UiFont`] is ever needed.
const EGUI_HAS_DEFAULT_FONTS: bool = cfg!(not(target_arch = "wasm32"));

#[derive(Default)]
struct Provided {
    files: std::collections::BTreeMap<UiFont, &'static [u8]>,
    /// Counts the changes, so each egui context installs the set again once.
    version: u32,
}

thread_local! {
    static PROVIDED: std::cell::RefCell<Provided> = std::cell::RefCell::new(Provided::default());
}

/// Gives the UI the file of a [`UiFont`]. The next frame installs it. The bytes stay for the life of the
/// page (the font is installed again after a GPU loss).
pub fn provide_font(font: UiFont, bytes: Vec<u8>) {
    PROVIDED.with_borrow_mut(|p| {
        if let std::collections::btree_map::Entry::Vacant(slot) = p.files.entry(font) {
            slot.insert(Box::leak(bytes.into_boxed_slice()));
            p.version += 1;
        }
    });
}

/// The UI fonts this build needs and does not have yet.
pub fn wanted_fonts() -> Vec<UiFont> {
    if EGUI_HAS_DEFAULT_FONTS {
        return Vec::new();
    }
    PROVIDED.with_borrow(|p| UiFont::ALL.into_iter().filter(|f| !p.files.contains_key(f)).collect())
}

/// The font files of the hand-written headings and the font picker: the same statics the typesetter registers, so
/// the binary holds each of them once.
fn hand_fonts() -> [(&'static str, &'static [u8]); 3] {
    use drafftink_typeset::{FontFamily as Face, FontWeight, font_file};
    let file = |face: Face| font_file(&face, FontWeight::Regular).unwrap_or_default();
    [
        (HAND_FONT, file(Face::GelPen)),
        (PREVIEW_SERIF, file(Face::GelPenSerif)),
        (PREVIEW_VANILLA, file(Face::VanillaExtract)),
    ]
}

/// Every colour the surfaces use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// The dark variant.
    pub dark: bool,
    /// Fill of panels and dialogs.
    pub panel: Color32,
    /// Border of panels.
    pub border: Color32,
    /// Normal text.
    pub text: Color32,
    /// Secondary text and section labels.
    pub muted: Color32,
    /// Selected tool, primary button, links.
    pub accent: Color32,
    /// Text on the accent colour.
    pub on_accent: Color32,
    /// Fill of a button under the pointer.
    pub hover: Color32,
    /// Fill of a button at rest inside a panel.
    pub button: Color32,
    /// Fill of a selected toggle that is not a tool.
    pub selected: Color32,
    /// Separator lines.
    pub separator: Color32,
    /// Dimming behind a dialog.
    pub backdrop: Color32,
    /// Errors and the leave button.
    pub danger: Color32,
    /// Connected.
    pub ok: Color32,
    /// Connecting.
    pub warn: Color32,
    /// Drop shadow.
    pub shadow: Color32,
}

impl Palette {
    /// The light palette: the old app's panel colours.
    pub const LIGHT: Palette = Palette {
        dark: false,
        panel: Color32::from_rgba_unmultiplied_const(250, 250, 252, 250),
        border: Color32::from_gray(220),
        text: Color32::from_gray(50),
        muted: Color32::from_gray(110),
        accent: Color32::from_rgb(59, 130, 246),
        on_accent: Color32::WHITE,
        hover: Color32::from_gray(238),
        button: Color32::from_gray(245),
        selected: Color32::from_rgb(219, 234, 254),
        separator: Color32::from_gray(225),
        backdrop: Color32::from_black_alpha(80),
        danger: Color32::from_rgb(239, 68, 68),
        ok: Color32::from_rgb(34, 197, 94),
        warn: Color32::from_rgb(250, 204, 21),
        shadow: Color32::from_black_alpha(28),
    };

    /// The dark palette.
    pub const DARK: Palette = Palette {
        dark: true,
        panel: Color32::from_rgba_unmultiplied_const(32, 34, 40, 250),
        border: Color32::from_gray(64),
        text: Color32::from_gray(228),
        muted: Color32::from_gray(150),
        accent: Color32::from_rgb(77, 146, 255),
        on_accent: Color32::WHITE,
        hover: Color32::from_gray(58),
        button: Color32::from_gray(46),
        selected: Color32::from_rgb(38, 64, 110),
        separator: Color32::from_gray(66),
        backdrop: Color32::from_black_alpha(140),
        danger: Color32::from_rgb(248, 113, 113),
        ok: Color32::from_rgb(74, 222, 128),
        warn: Color32::from_rgb(250, 204, 21),
        shadow: Color32::from_black_alpha(90),
    };

    /// The palette of the current egui theme.
    pub fn of(ctx: &Context) -> Palette {
        match ctx.theme() {
            Theme::Dark => Palette::DARK,
            Theme::Light => Palette::LIGHT,
        }
    }

    /// The stroke of a panel border.
    pub fn border_stroke(&self) -> Stroke {
        Stroke::new(1.0, self.border)
    }
}

/// The egui theme preference for the value of the `theme` setting. Unknown values follow the system.
pub fn theme_preference(setting: Option<&str>) -> ThemePreference {
    match setting {
        Some("dark") => ThemePreference::Dark,
        Some("light") => ThemePreference::Light,
        _ => ThemePreference::System,
    }
}

thread_local! {
    static HAND_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A font id in the hand-written family (GelPen). Falls back to the default font when the families are
/// not installed in this pass yet; egui would panic on a family without fonts. [`ensure_fonts`] sets
/// which case applies at the start of every `draw`.
pub fn hand(size: f32) -> FontId {
    if HAND_READY.with(std::cell::Cell::get) {
        FontId::new(size, FontFamily::Name(HAND_FONT.into()))
    } else {
        FontId::proportional(size)
    }
}

/// A font id for a named family of this module (`PREVIEW_SERIF`, `PREVIEW_VANILLA`), with the same
/// fallback as [`hand`].
pub fn named(family: &str, size: f32) -> FontId {
    if HAND_READY.with(std::cell::Cell::get) {
        FontId::new(size, FontFamily::Name(family.into()))
    } else {
        FontId::proportional(size)
    }
}

/// True when the GelPen families exist in this egui context.
fn fonts_present(ctx: &Context) -> bool {
    ctx.fonts(|f| f.definitions().families.contains_key(&FontFamily::Name(HAND_FONT.into())))
}

/// Installs the fonts when they are missing, or when a [`UiFont`] arrived since they were installed, and
/// records whether they can be used in this pass. Called at the start of every `draw`.
pub fn ensure_fonts(ctx: &Context) -> bool {
    let current = PROVIDED.with_borrow(|p| p.version);
    let installed = ctx.data(|d| d.get_temp::<u32>(egui::Id::unique("drafftink-fonts-version")));
    if !fonts_present(ctx) || installed != Some(current) {
        install_fonts(ctx);
    }
    let ready = fonts_present(ctx);
    HAND_READY.with(|r| r.set(ready));
    ready
}

/// Installs the GelPen fonts as named families. Call once per egui context; a new context (after a GPU
/// loss) needs it again.
pub fn install_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    let version = PROVIDED.with_borrow(|p| {
        // Where egui has no fonts of its own (the web), the loaded files make up its two lists.
        let mut chain = |family: FontFamily, order: &[UiFont]| {
            for font in order.iter().filter(|f| p.files.contains_key(f)) {
                let name = font.key().to_owned();
                if let Some(bytes) = p.files.get(font) {
                    fonts.font_data.insert(name.clone(), std::sync::Arc::new(FontData::from_static(bytes)));
                }
                fonts.families.entry(family.clone()).or_default().push(name);
            }
        };
        chain(FontFamily::Proportional, &[UiFont::UbuntuLight, UiFont::EmojiIcons]);
        chain(FontFamily::Monospace, &[UiFont::Hack, UiFont::UbuntuLight, UiFont::EmojiIcons]);
        p.version
    });
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("drafftink-fonts-version"), version));
    for (name, bytes) in hand_fonts() {
        fonts.font_data.insert(name.to_owned(), std::sync::Arc::new(FontData::from_static(bytes)));
        // The default proportional fonts follow as the fallback for glyphs GelPen lacks.
        let mut chain = vec![name.to_owned()];
        chain.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
        fonts.families.insert(FontFamily::Name(name.into()), chain);
    }
    // Where no UI font is there (the web before a file arrived, or after a failed download) GelPen draws the
    // text, so that a label never turns into boxes.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        if list.is_empty() {
            list.push(HAND_FONT.to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// Sets the theme preference and the visuals of both themes. Cheap to skip: the caller does it only when
/// the setting changed.
pub fn apply(ctx: &Context, setting: Option<&str>) {
    for (theme, pal) in [(Theme::Light, Palette::LIGHT), (Theme::Dark, Palette::DARK)] {
        let mut v = match theme {
            Theme::Light => egui::Visuals::light(),
            Theme::Dark => egui::Visuals::dark(),
        };
        v.selection.bg_fill = pal.accent;
        v.selection.stroke = Stroke::new(1.0, pal.on_accent);
        v.hyperlink_color = pal.accent;
        v.window_fill = pal.panel;
        v.panel_fill = pal.panel;
        v.window_stroke = pal.border_stroke();
        v.window_corner_radius = CornerRadius::same(DIALOG_RADIUS);
        v.menu_corner_radius = CornerRadius::same(PANEL_RADIUS);
        v.extreme_bg_color = if pal.dark { Color32::from_gray(24) } else { Color32::WHITE };
        v.override_text_color = Some(pal.text);
        let r = CornerRadius::same(BUTTON_RADIUS);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.separator);
        v.widgets.inactive.bg_fill = pal.button;
        v.widgets.inactive.weak_bg_fill = pal.button;
        v.widgets.hovered.bg_fill = pal.hover;
        v.widgets.hovered.weak_bg_fill = pal.hover;
        v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: pal.shadow };
        ctx.set_visuals_of(theme, v);
    }
    ctx.set_theme(theme_preference(setting));
}

#[cfg(test)]
mod tests {
    use drafftink_typeset::{FontFamily, FontWeight, font_file};

    #[test]
    fn the_hand_fonts_are_the_typesetters_statics() {
        let want = [FontFamily::GelPen, FontFamily::GelPenSerif, FontFamily::VanillaExtract];
        for ((_, bytes), family) in super::hand_fonts().into_iter().zip(want) {
            let theirs = font_file(&family, FontWeight::Regular).unwrap_or_default();
            assert!(!bytes.is_empty());
            assert!(std::ptr::eq(bytes, theirs), "{family:?}: the UI must use the typesetter's bytes, not a copy");
        }
    }
}
