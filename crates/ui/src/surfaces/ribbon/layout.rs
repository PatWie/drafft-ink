//! How the parts of the strip right of its left cluster share the room. Pure arithmetic, so it is tested without a
//! window.

/// The widths the planner needs, in points.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Widths<'a> {
    /// The colour group, two rows of swatches.
    pub colour: f32,
    /// The style button that stands for the colour group when it does not fit.
    pub colour_button: f32,
    /// Each contextual group, in order.
    pub groups: &'a [f32],
    /// The "More" button.
    pub more_button: f32,
    /// The "More" menu has properties of its own (sloppiness, line height and the like), so the button stays even
    /// when every group fits.
    pub extras: bool,
}

/// What is drawn inline, and what goes behind the "More" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fit {
    /// The colour group is drawn in the strip (else the style button stands for it).
    pub colour_inline: bool,
    /// The first `shown` groups are in the strip; the ones after them are in the "More" popover.
    pub shown: usize,
    /// The "More" button is there.
    pub more: bool,
    /// Even so the strip is wider than the room, so it scrolls sideways.
    pub scroll: bool,
}

/// Plans the strip for `room` points (the width right of the left cluster), with `divider` before every part.
///
/// The groups keep their order. When they do not fit, the ones at the right end go to the "More" popover first.
/// When the colour group does not fit with the "More" button, it collapses to the style button. A phone-sized
/// window (`narrow`) draws neither colour group nor contextual groups inline. A last resort is a sideways scroll.
pub(crate) fn fit(widths: Widths<'_>, divider: f32, room: f32, narrow: bool) -> Fit {
    let n = widths.groups.len();
    let more_cost = divider + widths.more_button;
    let has_more = |shown: usize| shown < n || widths.extras;
    // What the strip takes with `colour` before the groups and `shown` groups after it.
    let need = |colour: f32, shown: usize| -> f32 {
        let groups: f32 = widths.groups[..shown].iter().map(|w| divider + w).sum();
        divider + colour + groups + if has_more(shown) { more_cost } else { 0.0 }
    };
    let colour_inline = !narrow && need(widths.colour, 0) <= room;
    let colour = if colour_inline { widths.colour } else { widths.colour_button };
    let shown = if narrow { 0 } else { (0..=n).rev().find(|&k| need(colour, k) <= room).unwrap_or(0) };
    Fit { colour_inline, shown, more: has_more(shown), scroll: need(colour, shown) > room }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIVIDER: f32 = 13.0;
    const MORE: f32 = 64.0;

    fn widths(groups: &[f32], extras: bool) -> Widths<'_> {
        Widths { colour: 250.0, colour_button: 28.0, groups, more_button: MORE, extras }
    }

    #[test]
    fn everything_inline_when_it_fits_and_no_more_button_without_extras() {
        let fit = fit(widths(&[100.0, 100.0], false), DIVIDER, 1000.0, false);
        assert_eq!(fit, Fit { colour_inline: true, shown: 2, more: false, scroll: false });
    }

    #[test]
    fn the_more_button_stays_while_it_has_properties_of_its_own() {
        let fit = fit(widths(&[100.0], true), DIVIDER, 1000.0, false);
        assert!(fit.more && fit.shown == 1);
    }

    #[test]
    fn the_groups_at_the_right_end_go_to_more_first_and_the_order_is_kept() {
        // Colour 250 + 13, groups of 100 (+13 each), More 64 + 13: three groups need 263 + 339 + 77 = 679.
        let all = [100.0; 4];
        let fit = fit(widths(&all, false), DIVIDER, 700.0, false);
        assert_eq!((fit.shown, fit.more, fit.colour_inline, fit.scroll), (3, true, true, false));
        let tighter = super::fit(widths(&all, false), DIVIDER, 500.0, false);
        assert_eq!(tighter.shown, 1, "the first groups stay: {tighter:?}");
    }

    #[test]
    fn the_colour_group_collapses_when_it_cannot_stand_with_the_more_button() {
        let fit = fit(widths(&[100.0, 100.0], false), DIVIDER, 250.0, false);
        assert!(!fit.colour_inline && fit.more && fit.shown == 1, "{fit:?}");
        assert!(!fit.scroll, "the button, one group and More fit");
    }

    #[test]
    fn a_window_too_narrow_even_for_that_scrolls() {
        let fit = fit(widths(&[100.0], false), DIVIDER, 60.0, false);
        assert!(fit.scroll && !fit.colour_inline);
    }

    #[test]
    fn a_phone_keeps_everything_behind_buttons() {
        let fit = fit(widths(&[100.0, 100.0], false), DIVIDER, 5000.0, true);
        assert_eq!(fit, Fit { colour_inline: false, shown: 0, more: true, scroll: false });
    }

    #[test]
    fn no_groups_and_no_extras_leave_no_more_button() {
        let fit = fit(widths(&[], false), DIVIDER, 1000.0, false);
        assert_eq!(fit, Fit { colour_inline: true, shown: 0, more: false, scroll: false });
    }
}
