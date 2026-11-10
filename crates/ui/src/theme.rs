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

static GELPEN: &[u8] = include_bytes!("../../typeset/assets/fonts/GelPen.ttf");
static GELPEN_SERIF: &[u8] = include_bytes!("../../typeset/assets/fonts/GelPenSerifMedium.ttf");
static VANILLA: &[u8] = include_bytes!("../../typeset/assets/fonts/VanillaExtract.ttf");

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

/// A font id in the hand-written family.
pub fn hand(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(HAND_FONT.into()))
}

/// Installs the GelPen fonts as named families. Call once per egui context; a new context (after a GPU
/// loss) needs it again.
pub fn install_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [(HAND_FONT, GELPEN), (PREVIEW_SERIF, GELPEN_SERIF), (PREVIEW_VANILLA, VANILLA)] {
        fonts.font_data.insert(name.to_owned(), std::sync::Arc::new(FontData::from_static(bytes)));
        // The default proportional fonts follow as the fallback for glyphs GelPen lacks.
        let mut chain = vec![name.to_owned()];
        chain.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
        fonts.families.insert(FontFamily::Name(name.into()), chain);
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
