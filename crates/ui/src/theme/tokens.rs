//! Design tokens: the named sizes every surface draws with, so the chrome looks like one thing.
//!
//! * [`TextSize`] is the type scale. A surface asks for `TextSize::Body`, never for `13.0`
//!   (`tests/tokens.rs` fails on a new literal font size in `surfaces/`).
//! * [`space`] is the spacing scale, [`radius`] the corner ladder.
//! * [`Control`] is the ladder of control heights. The touch layout has its own, larger heights
//!   ([`Control::points`]).

use egui::{FontId, RichText};

/// The type scale. Every size the chrome uses is one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSize {
    /// Keyboard hints, the smallest print.
    Micro,
    /// Secondary labels and shortcuts.
    Caption,
    /// Table rows, small buttons and field text.
    Small,
    /// Menu rows, buttons and body text.
    Body,
    /// Section captions and small headings.
    Label,
    /// Panel titles.
    Heading,
    /// Dialog titles.
    Title,
    /// Large headings (the About dialog).
    Display,
}

impl TextSize {
    /// The size in points.
    pub const fn points(self) -> f32 {
        match self {
            Self::Micro => 10.0,
            Self::Caption => 11.0,
            Self::Small => 12.0,
            Self::Body => 13.0,
            Self::Label => 14.0,
            Self::Heading => 16.0,
            Self::Title => 18.0,
            Self::Display => 22.0,
        }
    }

    /// The UI font at this size (the user's choice, GelPen by default; see `theme::UiFontChoice`).
    pub fn font(self) -> FontId {
        FontId::proportional(self.points())
    }

    /// GelPen at this size, for headings that keep the hand-drawn look whatever the UI font is.
    pub fn hand(self) -> FontId {
        super::hand(self.points())
    }

    /// The monospace font at this size (code and numbers).
    pub fn mono(self) -> FontId {
        FontId::monospace(self.points())
    }

    /// Rich text at this size in the UI font.
    pub fn rich(self, text: impl Into<String>) -> RichText {
        RichText::new(text).size(self.points())
    }
}

/// The spacing scale, in points: gaps between controls, padding of bars.
pub mod space {
    /// Between a button and its own ring or shadow.
    pub const XXS: f32 = 2.0;
    /// Between the tools of a column.
    pub const XS: f32 = 3.0;
    /// Between controls of one group.
    pub const SM: f32 = 4.0;
    /// Around a separator, between small groups, above and below a bar.
    pub const MD: f32 = 6.0;
    /// Padding inside a bar, and the gap between a bar and what opens next to it.
    pub const LG: f32 = 8.0;
    /// Between groups, and the distance of a floating panel from the window edge.
    pub const XL: f32 = 12.0;
    /// Padding of a dialog section.
    pub const XXL: f32 = 16.0;
}

/// The ladder of corner radii, in points.
pub mod radius {
    /// Hairline controls (a text field).
    pub const SM: u8 = 4;
    /// Buttons and rows.
    pub const MD: u8 = 6;
    /// Panels, bars and menus.
    pub const LG: u8 = 8;
    /// Dialogs.
    pub const XL: u8 = 12;
}

/// The heights of controls. A bar picks one rung for all its buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// A text toggle, a colour swatch cell.
    Toggle,
    /// An icon button in a bar, a menu row.
    Small,
    /// A dialog button.
    Button,
    /// A tool button in the tool column.
    Tool,
}

impl Control {
    /// The height (and, for an icon button, the side) in points. `touch` is the touch layout, which has
    /// room for fingers.
    pub const fn points(self, touch: bool) -> f32 {
        match (self, touch) {
            (Self::Toggle, false) => 24.0,
            (Self::Toggle, true) => 36.0,
            (Self::Small, false) => 28.0,
            (Self::Small, true) => 40.0,
            (Self::Button, false) => 32.0,
            (Self::Button, true) => 44.0,
            (Self::Tool, false) => 34.0,
            (Self::Tool, true) => 44.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_type_scale_grows_with_the_name() {
        let scale = [
            TextSize::Micro,
            TextSize::Caption,
            TextSize::Small,
            TextSize::Body,
            TextSize::Label,
            TextSize::Heading,
            TextSize::Title,
            TextSize::Display,
        ];
        assert!(scale.windows(2).all(|pair| pair[0].points() < pair[1].points()));
    }

    #[test]
    fn touch_controls_are_never_smaller() {
        for control in [Control::Toggle, Control::Small, Control::Button, Control::Tool] {
            assert!(control.points(true) > control.points(false), "{control:?}");
        }
    }
}
