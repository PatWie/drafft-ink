//! How the groups of the ribbon are packed into rows. Pure arithmetic, so it is tested without a window.

/// The groups of one row, as indices into the list of visible groups, and the width the row takes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Row {
    pub groups: Vec<usize>,
    pub width: f32,
}

/// How the ribbon is laid out.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Plan {
    /// Rows one under the other; the groups keep their order from left to right, then top to bottom.
    Rows(Vec<Row>),
    /// One row that scrolls sideways: the window is narrower than the widest group, or the layout is the phone's.
    Scroll(Row),
}

impl Plan {
    /// The width of the widest row.
    pub(super) fn width(&self) -> f32 {
        match self {
            Plan::Rows(rows) => rows.iter().map(|r| r.width).fold(0.0, f32::max),
            Plan::Scroll(row) => row.width,
        }
    }
}

/// Packs groups of the given `widths` into rows no wider than `available`, with `divider` between two groups of a
/// row. A row is filled before the next one starts, so the order of the groups never changes. When one group
/// alone is wider than `available`, or `single_row` is set, the answer is one scrolling row.
pub(super) fn pack(widths: &[f32], divider: f32, available: f32, single_row: bool) -> Plan {
    let one_row = |widths: &[f32]| Row {
        groups: (0..widths.len()).collect(),
        width: widths.iter().sum::<f32>() + divider * widths.len().saturating_sub(1) as f32,
    };
    if single_row || widths.iter().any(|w| *w > available) {
        return Plan::Scroll(one_row(widths));
    }
    let mut rows: Vec<Row> = Vec::new();
    for (index, width) in widths.iter().enumerate() {
        match rows.last_mut() {
            Some(row) if row.width + divider + width <= available => {
                row.groups.push(index);
                row.width += divider + width;
            }
            _ => rows.push(Row { groups: vec![index], width: *width }),
        }
    }
    if rows.len() == 1 { Plan::Rows(rows) } else { balance(rows, widths, divider) }
}

/// Moves the last groups of a row down when the row below is much shorter, so that the rows are about the same
/// width and the card does not look like a long line with a stub under it. The order is kept.
fn balance(mut rows: Vec<Row>, widths: &[f32], divider: f32) -> Plan {
    let row_width = |groups: &[usize]| {
        groups.iter().map(|&g| widths[g]).sum::<f32>() + divider * groups.len().saturating_sub(1) as f32
    };
    // Only the last two rows are balanced against each other; moving a group never makes the longer row longer.
    while let [.., upper, lower] = rows.as_mut_slice() {
        let Some(&moved) = upper.groups.last() else { break };
        if upper.groups.len() < 2 {
            break;
        }
        let new_upper = row_width(&upper.groups[..upper.groups.len() - 1]);
        let new_lower = lower.width + divider + widths[moved];
        if new_lower.max(new_upper) >= upper.width.max(lower.width) {
            break;
        }
        upper.groups.pop();
        upper.width = new_upper;
        lower.groups.insert(0, moved);
        lower.width = new_lower;
    }
    Plan::Rows(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIVIDER: f32 = 13.0;

    fn rows(plan: &Plan) -> Vec<Vec<usize>> {
        match plan {
            Plan::Rows(rows) => rows.iter().map(|r| r.groups.clone()).collect(),
            Plan::Scroll(_) => panic!("expected rows, got {plan:?}"),
        }
    }

    #[test]
    fn groups_that_fit_make_one_row_with_dividers_between() {
        let plan = pack(&[100.0, 200.0, 50.0], DIVIDER, 1000.0, false);
        assert_eq!(rows(&plan), [vec![0, 1, 2]]);
        assert_eq!(plan.width(), 350.0 + 2.0 * DIVIDER);
    }

    #[test]
    fn a_row_ends_where_the_next_group_does_not_fit_and_the_order_is_kept() {
        let plan = pack(&[300.0, 300.0, 300.0, 300.0], DIVIDER, 640.0, false);
        assert_eq!(rows(&plan), [vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn a_stub_row_takes_a_group_from_the_row_above() {
        // 500 + 13 + 100 + 13 + 400 fits no row of 640: first-fit gives [500, 100] and [400]; balanced is better.
        let plan = pack(&[500.0, 100.0, 120.0], DIVIDER, 640.0, false);
        let widths = |plan: &Plan| match plan {
            Plan::Rows(rows) => rows.iter().map(|r| r.width).collect::<Vec<_>>(),
            Plan::Scroll(_) => Vec::new(),
        };
        assert_eq!(rows(&plan), [vec![0], vec![1, 2]]);
        assert!(widths(&plan).iter().all(|w| *w <= 640.0));
    }

    #[test]
    fn balancing_never_makes_the_widest_row_wider() {
        let widths = [300.0, 280.0, 60.0];
        let plan = pack(&widths, DIVIDER, 640.0, false);
        let first_fit = 300.0 + DIVIDER + 280.0;
        assert!(plan.width() <= first_fit);
    }

    #[test]
    fn a_group_wider_than_the_room_makes_one_scrolling_row() {
        let plan = pack(&[100.0, 700.0], DIVIDER, 640.0, false);
        assert!(matches!(&plan, Plan::Scroll(row) if row.groups == [0, 1]));
        assert_eq!(plan.width(), 800.0 + DIVIDER);
    }

    #[test]
    fn the_phone_layout_always_scrolls_one_row() {
        assert!(matches!(pack(&[50.0, 50.0], DIVIDER, 640.0, true), Plan::Scroll(_)));
    }

    #[test]
    fn no_groups_make_no_rows() {
        assert_eq!(pack(&[], DIVIDER, 640.0, false), Plan::Rows(Vec::new()));
    }
}
