#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Cost and limits: presence follows the budget, bundles are bounded, rows written stay low.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, JoinMode, ServerMessage};
use drafftink_protocol::{PRESENCE_TICK_MS, ROOM_PRESENCE_BUDGET};
use drafftink_relay::ConnId;

fn limit_of(messages: &[ServerMessage]) -> u16 {
    messages
        .iter()
        .find_map(
            |m| if let ServerMessage::Joined { limits, .. } = m { Some(limits.max_presence_per_sec) } else { None },
        )
        .expect("joined")
}

#[test]
fn the_personal_presence_rate_follows_the_budget_as_people_join() {
    let mut h = Harness::self_host();
    let mut rates = Vec::new();
    for n in 1..=50u8 {
        let conn = h.join(client(n), JoinMode::Create, 0);
        rates.push(limit_of(&h.take(conn)));
    }
    assert_eq!(&rates[..4], [20, 20, 13, 10]);
    assert_eq!(rates[29], 1, "30 people: 40/30 rounds down to 1");
    assert_eq!(rates[49], 1);
    assert!(rates.windows(2).all(|w| w[0] >= w[1]), "never grows with more people");
}

#[test]
fn peers_are_told_the_new_rate_when_somebody_joins() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    for n in 2..=4 {
        h.open(n);
    }
    let told: Vec<u16> = h
        .take(a)
        .iter()
        .filter_map(|m| {
            if let ServerMessage::PeerJoined { max_presence_per_sec, .. } = m {
                Some(*max_presence_per_sec)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(told, [20, 13, 10]);
}

/// 50 people, everybody sends 20 messages a second for ten simulated seconds.
#[test]
fn fifty_people_sending_flat_out_stay_within_the_room_budget() {
    let mut h = Harness::self_host();
    let conns: Vec<ConnId> = (1..=50u8).map(|n| h.open(n)).collect();
    for conn in &conns {
        h.take(*conn);
    }
    h.advance(10 * SECOND);
    let mut delivered_per_second = [0u32; 10];
    for delivered in &mut delivered_per_second {
        for _tick in 0..(1000 / 50) {
            for (n, conn) in conns.iter().enumerate() {
                let sender = u8::try_from(n + 1).expect("fits");
                h.send(*conn, &ClientMessage::Presence { record: presence(client(sender)) });
            }
            h.advance(50);
        }
        // Count what the relay accepted: every accepted record is in some bundle for the first connection's peers.
        let bundles = h.take(conns[0]);
        *delivered = bundles
            .iter()
            .map(|m| {
                if let ServerMessage::PresenceBundle(entries) = m {
                    u32::try_from(entries.len()).expect("fits")
                } else {
                    0
                }
            })
            .sum();
    }
    let budget = u32::from(ROOM_PRESENCE_BUDGET);
    for (second, delivered) in delivered_per_second.iter().enumerate() {
        assert!(
            *delivered <= budget,
            "second {second}: {delivered} presence messages reached one person, the budget is {budget}"
        );
    }
    assert!(delivered_per_second.iter().sum::<u32>() > 0, "presence does flow");
}

#[test]
fn presence_is_bundled_at_most_once_per_tick_and_never_echoed() {
    let mut h = Harness::self_host();
    let (a, b, c) = (h.open(1), h.open(2), h.open(3));
    for conn in [a, b, c] {
        h.take(conn);
    }
    h.advance(1000);
    h.send(a, &ClientMessage::Presence { record: presence(client(1)) });
    h.send(b, &ClientMessage::Presence { record: presence(client(2)) });
    h.advance(u64::from(PRESENCE_TICK_MS));
    let for_c = h.take(c);
    let [ServerMessage::PresenceBundle(entries)] = &for_c[..] else { panic!("{for_c:?}") };
    assert_eq!(entries.len(), 2, "one bundle with both");
    let for_a = h.take(a);
    let [ServerMessage::PresenceBundle(entries)] = &for_a[..] else { panic!("{for_a:?}") };
    assert_eq!(entries.iter().map(|e| e.from).collect::<Vec<_>>(), [client(2)], "not echoed to the sender");
}

#[test]
fn only_the_newest_presence_of_a_person_is_kept_for_the_bundle() {
    let mut h = Harness::self_host();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    h.advance(1000);
    let first = record(drafftink_protocol::v2::RecordKind::Presence, client(1), 1, 0, 10);
    let second = record(drafftink_protocol::v2::RecordKind::Presence, client(1), 2, 0, 11);
    h.send(a, &ClientMessage::Presence { record: first });
    h.send(a, &ClientMessage::Presence { record: second.clone() });
    h.advance(200);
    let for_b = h.take(b);
    let [ServerMessage::PresenceBundle(entries)] = &for_b[..] else { panic!("{for_b:?}") };
    assert_eq!(entries[0].record, second);
}

#[test]
fn nobody_hears_presence_of_a_person_who_is_alone() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    h.send(a, &ClientMessage::Presence { record: presence(client(1)) });
    h.advance(SECOND);
    assert!(h.take(a).is_empty());
    assert_eq!(h.wake_at, None, "nothing is waiting");
}

#[test]
fn presence_is_never_stored() {
    let mut h = Harness::self_host();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    for _ in 0..20 {
        h.send(a, &ClientMessage::Presence { record: presence(client(1)) });
        h.advance(100);
    }
    assert_eq!(h.store.segments_written(), 0);
    assert!(h.store.dump().is_empty());
    assert!(h.ops.iter().all(|op| matches!(op, drafftink_relay::StoreOp::SaveMeta)));
}

#[test]
fn rows_written_for_a_busy_minute_stay_in_the_tens_not_the_thousands() {
    let mut h = Harness::new(demo());
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    let before = h.rows_written();
    let mut ctr = 0;
    for _ in 0..600 {
        ctr += 1;
        h.send(a, &ClientMessage::Append { record: update(client(1), ctr) });
        h.advance(100);
    }
    let rows = h.rows_written() - before;
    assert!(rows <= 62, "600 edits in a minute wrote {rows} rows");
    assert!(rows >= 55, "one segment per second window: {rows}");
}
