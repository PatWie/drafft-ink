//! Whether a pointer gesture binds the end of an arrow to a shape: the one place that combines the setting
//! `arrow.binding` with the keys held.
//!
//! The setting is the person's standing choice. Holding Ctrl (Cmd on a Mac) turns it around for the gesture in
//! progress, as in Excalidraw: with binding on, Ctrl keeps the end free; with binding off, Ctrl binds. Alt binds
//! inside the shape, but only when binding is in effect. The gesture then names its choice as an explicit
//! argument (`on`, `off` or `inside`), so `arrow.move_point` and `element.create` never see the key and the
//! ring entry replays the same arrow. The commands themselves are not blocked by the setting: `bind: "on"` and
//! `bind: "inside"` bind with the setting off.

use drafftink_canvas::Modifiers;

use crate::cmd::arrow::BindChoice;

/// The choice a gesture sends, given the setting `arrow.binding` and the keys held now.
pub(crate) fn choice(binding_on: bool, mods: Modifiers) -> BindChoice {
    // Ctrl or Cmd inverts the setting.
    let in_effect = binding_on != mods.command();
    match (in_effect, mods.alt) {
        (false, _) => BindChoice::Off,
        (true, true) => BindChoice::Inside,
        (true, false) => BindChoice::On,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Modifiers = Modifiers { ctrl: true, ..Modifiers::NONE };
    const META: Modifiers = Modifiers { meta: true, ..Modifiers::NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..Modifiers::NONE };
    const CTRL_ALT: Modifiers = Modifiers { ctrl: true, alt: true, ..Modifiers::NONE };

    #[test]
    fn with_binding_on_ctrl_keeps_the_end_free_and_alt_binds_inside() {
        assert_eq!(choice(true, Modifiers::NONE), BindChoice::On);
        assert_eq!(choice(true, CTRL), BindChoice::Off);
        assert_eq!(choice(true, META), BindChoice::Off);
        assert_eq!(choice(true, ALT), BindChoice::Inside);
        assert_eq!(choice(true, CTRL_ALT), BindChoice::Off);
    }

    #[test]
    fn with_binding_off_ctrl_binds_and_alone_alt_does_not() {
        assert_eq!(choice(false, Modifiers::NONE), BindChoice::Off);
        assert_eq!(choice(false, CTRL), BindChoice::On);
        assert_eq!(choice(false, META), BindChoice::On);
        assert_eq!(choice(false, ALT), BindChoice::Off);
        assert_eq!(choice(false, CTRL_ALT), BindChoice::Inside);
    }
}
