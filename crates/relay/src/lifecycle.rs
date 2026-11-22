//! The retention state machine (design section 10.2). It reads no clock: the host passes the time.
//!
//! ```text
//! visit (an accepted Join) : deadline = now + ttl; not expiring; grace cancelled
//! last connection closed   : grace_until = now + grace
//! check(now, live)         : now < deadline                 -> keep, wake at the deadline
//!                            live > 0                       -> expiring, wake after `recheck`
//!                            no one live, inside the grace  -> keep, wake at the end of the grace
//!                            no one live, outside the grace -> delete
//! ```
//!
//! "Live" is decided by the caller: a connection counts only when it was heard from recently, so a socket that died
//! without a close event does not keep a room for ever.

use crate::config::Retention;

/// What to do with a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The room stays.
    Keep {
        /// When to check again. `None` when the room never expires.
        wake_at: Option<u64>,
        /// The room is past its deadline and will be deleted when the last person leaves.
        expiring: bool,
    },
    /// Delete the room now.
    Delete,
}

/// The retention state of one room, besides the deadline that its meta stores.
#[derive(Debug, Clone, Copy)]
pub struct Lifecycle {
    policy: Retention,
    expiring: bool,
    grace_until: Option<u64>,
}

impl Lifecycle {
    /// A room under `policy`.
    pub const fn new(policy: Retention) -> Self {
        Lifecycle { policy, expiring: false, grace_until: None }
    }

    /// Whether the room is past its deadline with people still in it.
    pub const fn expiring(&self) -> bool {
        self.expiring
    }

    /// The deadline after a visit at `now_ms`. 0 means the room never expires.
    pub fn deadline_after_visit(&self, now_ms: u64) -> u64 {
        match self.policy {
            Retention::Never => 0,
            Retention::After { ttl, .. } => now_ms.saturating_add(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX)),
        }
    }

    /// A visit: the room is not expiring any more, and no deletion is waiting for the grace. Returns whether the
    /// room had been expiring, so that the others can be told.
    pub fn visited(&mut self) -> bool {
        self.grace_until = None;
        std::mem::take(&mut self.expiring)
    }

    /// A connection closed. When none is left the room gets its grace.
    pub fn connection_closed(&mut self, now_ms: u64, live_left: usize) {
        if let Retention::After { grace, .. } = self.policy
            && live_left == 0
        {
            let grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
            self.grace_until = Some(now_ms.saturating_add(grace_ms));
        }
    }

    /// Decides what to do at `now_ms`, with `live` connections heard from recently. `deadline_ms` is the room's
    /// stored deadline, 0 for none.
    pub fn check(&mut self, deadline_ms: u64, now_ms: u64, live: usize) -> Verdict {
        let Retention::After { recheck, .. } = self.policy else {
            return Verdict::Keep { wake_at: None, expiring: false };
        };
        if deadline_ms == 0 || now_ms < deadline_ms {
            self.expiring = false;
            return Verdict::Keep { wake_at: (deadline_ms != 0).then_some(deadline_ms), expiring: false };
        }
        if live > 0 {
            self.expiring = true;
            self.grace_until = None;
            let recheck_ms = u64::try_from(recheck.as_millis()).unwrap_or(u64::MAX);
            return Verdict::Keep { wake_at: Some(now_ms.saturating_add(recheck_ms)), expiring: true };
        }
        match self.grace_until {
            Some(until) if now_ms < until => Verdict::Keep { wake_at: Some(until), expiring: self.expiring },
            _ => Verdict::Delete,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const H: u64 = 3_600_000;

    fn demo() -> Lifecycle {
        Lifecycle::new(Retention::After {
            ttl: Duration::from_secs(48 * 3600),
            grace: Duration::from_secs(60),
            recheck: Duration::from_secs(3600),
        })
    }

    #[test]
    fn a_visit_sets_the_deadline_to_the_ttl_after_it() {
        assert_eq!(demo().deadline_after_visit(1_000), 1_000 + 48 * H);
        assert_eq!(Lifecycle::new(Retention::Never).deadline_after_visit(1_000), 0);
    }

    #[test]
    fn before_the_deadline_the_room_waits_for_it() {
        let mut l = demo();
        assert_eq!(l.check(48 * H, 10 * H, 0), Verdict::Keep { wake_at: Some(48 * H), expiring: false });
    }

    #[test]
    fn an_abandoned_room_is_deleted_at_the_deadline() {
        assert_eq!(demo().check(48 * H, 48 * H, 0), Verdict::Delete);
    }

    #[test]
    fn a_room_with_people_at_the_deadline_is_expiring_and_rechecked() {
        let mut l = demo();
        assert_eq!(l.check(48 * H, 48 * H, 2), Verdict::Keep { wake_at: Some(49 * H), expiring: true });
        assert!(l.expiring());
        assert_eq!(l.check(48 * H, 49 * H, 1), Verdict::Keep { wake_at: Some(50 * H), expiring: true });
    }

    #[test]
    fn the_last_person_leaving_an_expiring_room_starts_the_grace() {
        let mut l = demo();
        l.check(48 * H, 48 * H, 1);
        l.connection_closed(48 * H + 5_000, 0);
        let end = 48 * H + 65_000;
        assert_eq!(l.check(48 * H, 48 * H + 6_000, 0), Verdict::Keep { wake_at: Some(end), expiring: true });
        assert_eq!(l.check(48 * H, end, 0), Verdict::Delete);
    }

    #[test]
    fn a_rejoin_inside_the_grace_cancels_the_deletion() {
        let mut l = demo();
        l.check(48 * H, 48 * H, 1);
        l.connection_closed(48 * H + 5_000, 0);
        assert!(l.visited(), "the room had been expiring");
        let deadline = l.deadline_after_visit(48 * H + 30_000);
        assert_eq!(l.check(deadline, 48 * H + 70_000, 1), Verdict::Keep { wake_at: Some(deadline), expiring: false });
        assert!(!l.visited());
    }

    #[test]
    fn a_connection_that_closed_shortly_before_the_deadline_still_gets_its_grace() {
        let mut l = demo();
        l.connection_closed(48 * H - 120_000, 0);
        assert_eq!(l.check(48 * H, 48 * H, 0), Verdict::Delete, "the grace ended before the deadline");
        let mut l = demo();
        l.connection_closed(48 * H - 10_000, 0);
        assert_eq!(l.check(48 * H, 48 * H, 0), Verdict::Keep { wake_at: Some(48 * H + 50_000), expiring: false });
    }

    #[test]
    fn a_room_that_never_expires_is_never_checked() {
        let mut l = Lifecycle::new(Retention::Never);
        assert_eq!(l.check(0, u64::MAX, 0), Verdict::Keep { wake_at: None, expiring: false });
    }
}
