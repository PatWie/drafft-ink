#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Every input is hostile: random server messages, random bytes, random order and random time never panic, never make
//! the client ask to be woken in the past, and never reconnect a client that stopped.

mod common;

use common::*;
use drafftink_collab::*;
use drafftink_protocol::{
    AwarenessState, ErrorCode, Limits, PeerInfo, Point, ServerMessage, SyncKind, UserInfo, Viewport,
};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Tick(u64),
    Opened,
    Closed(bool),
    Raw(Vec<u8>),
    Msg(ServerMessage),
    Edit,
    Cursor(f64, f64),
    Select(u8),
    Follow(u8),
    StopFollowing,
    Leave,
    Start,
    Retry,
    Resolve(bool),
    /// A well-formed join: Opened, Joined and a SyncResponse of the given flavour.
    Join(u8),
    /// The server's document gets an element and relays it.
    ServerEdit,
    /// The server acknowledges the oldest update the client sent.
    AckLast,
}

fn client_id(n: u8) -> drafftink_protocol::ClientId {
    cid(n % 4 + 1)
}

fn user_strategy() -> impl Strategy<Value = Option<UserInfo>> {
    prop::option::of(("[a-z]{0,8}", 0u32..0x0100_0000).prop_map(|(name, color)| UserInfo { name, color }))
}

fn awareness() -> impl Strategy<Value = AwarenessState> {
    (
        user_strategy(),
        prop::option::of((-1e6f64..1e6, -1e6f64..1e6).prop_map(|(x, y)| Point { x, y })),
        prop::collection::vec("[a-z0-9]{1,6}", 0..4),
        prop::option::of(
            (-1e6f64..1e6, -1e6f64..1e6, 0f64..1e5, 0f64..1e5).prop_map(|(x, y, width, height)| Viewport {
                x,
                y,
                width,
                height,
            }),
        ),
    )
        .prop_map(|(user, cursor, selection, viewport)| AwarenessState { user, cursor, selection, viewport })
}

fn error_code() -> impl Strategy<Value = ErrorCode> {
    (1u8..=14).prop_map(|n| ErrorCode::from_u8(n).unwrap())
}

fn messages(blobs: Vec<Vec<u8>>) -> impl Strategy<Value = ServerMessage> {
    let data = prop_oneof![prop::collection::vec(any::<u8>(), 0..64), prop::sample::select(blobs)];
    let data2 = data.clone();
    let peers = prop::collection::vec(
        (any::<u8>(), user_strategy()).prop_map(|(n, user)| PeerInfo { client_id: client_id(n), user }),
        0..4,
    );
    prop_oneof![
        (peers, prop::collection::vec(any::<u8>(), 0..16), 0u16..3, any::<bool>()).prop_map(
            |(peers, server_version, v, right_room)| {
                ServerMessage::Joined {
                    protocol_version: v,
                    server: "fuzz".into(),
                    room: if right_room { room() } else { drafftink_protocol::RoomId::new("other").unwrap() },
                    client_id: cid(1),
                    peers,
                    server_version,
                    limits: Limits { max_update_bytes: 4096, ..Limits::DEFAULT },
                }
            }
        ),
        (any::<bool>(), data, prop::collection::vec(any::<u8>(), 0..16)).prop_map(|(snap, data, server_version)| {
            ServerMessage::SyncResponse {
                kind: if snap { SyncKind::Snapshot } else { SyncKind::Updates },
                data,
                server_version,
            }
        }),
        (any::<u8>(), data2).prop_map(|(n, update)| ServerMessage::Update {
            from: client_id(n),
            update: if update.is_empty() { vec![0] } else { update }
        }),
        any::<u32>().prop_map(|seq| ServerMessage::Ack { seq }),
        (any::<u8>(), user_strategy())
            .prop_map(|(n, user)| ServerMessage::PeerJoined { peer: PeerInfo { client_id: client_id(n), user } }),
        any::<u8>().prop_map(|n| ServerMessage::PeerLeft { client_id: client_id(n) }),
        (any::<u8>(), awareness()).prop_map(|(n, state)| ServerMessage::PeerAwareness { from: client_id(n), state }),
        (error_code(), prop::option::of(0u32..100_000)).prop_map(|(code, retry_after_ms)| ServerMessage::Error {
            code,
            message: "fuzz".into(),
            retry_after_ms
        }),
        any::<u64>().prop_map(|nonce| ServerMessage::Ping { nonce }),
        any::<u64>().prop_map(|nonce| ServerMessage::Pong { nonce }),
    ]
}

fn ops(blobs: Vec<Vec<u8>>) -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        6 => (0u64..40_000).prop_map(Op::Tick),
        3 => Just(Op::Opened),
        2 => any::<bool>().prop_map(Op::Closed),
        1 => prop::collection::vec(any::<u8>(), 0..64).prop_map(Op::Raw),
        14 => messages(blobs).prop_map(Op::Msg),
        3 => Just(Op::Edit),
        2 => (-1e6f64..1e6, -1e6f64..1e6).prop_map(|(x, y)| Op::Cursor(x, y)),
        1 => any::<u8>().prop_map(Op::Select),
        1 => any::<u8>().prop_map(Op::Follow),
        1 => Just(Op::StopFollowing),
        1 => Just(Op::Leave),
        2 => Just(Op::Start),
        1 => Just(Op::Retry),
        1 => any::<bool>().prop_map(Op::Resolve),
        8 => any::<u8>().prop_map(Op::Join),
        6 => Just(Op::ServerEdit),
        4 => Just(Op::AckLast),
    ];
    prop::collection::vec(op, 1..120)
}

/// Valid Loro payloads a hostile server might also send: a real update, a snapshot, a shallow snapshot.
fn blobs() -> Vec<Vec<u8>> {
    let mut s = common::store(100);
    add(&mut s, &["x", "y"]);
    let mut out = vec![s.export_snapshot().unwrap(), s.export_updates_since(&[]).unwrap()];
    for i in 0..3 {
        add(&mut s, &[&format!("z{i}")]);
    }
    out.push(s.export_shallow_snapshot(0).unwrap());
    out.push(s.export_state_only().unwrap());
    out
}

fn run(script: Vec<Op>, replace: bool) {
    let mut cfg = config("ws://127.0.0.1:3030");
    cfg.replace_when_trimmed = replace;
    let mut h = Harness::with(cfg);
    let mut counter = 0u32;
    let mut server_edits = 0u32;
    let mut last_seq = None;
    for op in script {
        let was_stopped = h.client.status().state == ConnState::Stopped;
        let connects = h.connects;
        let restarts = matches!(op, Op::Start);
        match op {
            Op::Tick(ms) => {
                h.now += ms;
                h.tick();
            }
            Op::Opened => {
                h.event(TransportEvent::Opened);
                h.tick();
            }
            Op::Closed(err) => {
                h.event(TransportEvent::Closed { error: err.then(|| "fuzz".to_owned()) });
                h.tick();
            }
            Op::Raw(bytes) => {
                h.event(TransportEvent::Message(bytes));
                h.tick();
            }
            Op::Msg(m) => {
                if let Ok(bytes) = m.encode() {
                    h.event(TransportEvent::Message(bytes));
                    h.tick();
                }
            }
            Op::Edit => {
                counter += 1;
                add(&mut h.store, &[&format!("l{counter}")]);
                h.client.local_changed(h.now);
                h.tick();
            }
            Op::Cursor(x, y) => {
                h.client.set_cursor(h.now, Some(Point { x, y }));
                h.tick();
            }
            Op::Select(n) => {
                h.client.set_selection((0..n % 5).map(|i| format!("s{i}")));
                h.tick();
            }
            Op::Follow(n) => {
                h.client.follow(client_id(n));
                h.tick();
            }
            Op::StopFollowing => h.client.stop_following(),
            Op::Leave => {
                h.client.leave(h.now, &mut h.store);
                h.drain();
            }
            Op::Start => {
                h.client.start(h.now);
                h.tick();
            }
            Op::Retry => {
                h.client.retry_now(h.now);
                h.tick();
            }
            Op::Join(kind) => {
                h.event(TransportEvent::Opened);
                let peers = if kind % 2 == 0 { vec![] } else { vec![PeerInfo { client_id: cid(2), user: None }] };
                let joined = h.joined(peers, h.server.version_vector());
                h.feed(&joined);
                let sync = match kind % 3 {
                    0 => h.sync_from(&h.store.version_vector()),
                    1 => ServerMessage::SyncResponse {
                        kind: SyncKind::Snapshot,
                        data: h.server.export_snapshot().unwrap(),
                        server_version: h.server.version_vector(),
                    },
                    _ => ServerMessage::SyncResponse {
                        kind: SyncKind::Snapshot,
                        data: h.server.export_shallow_snapshot(0).unwrap(),
                        server_version: h.server.version_vector(),
                    },
                };
                h.feed(&sync);
            }
            Op::ServerEdit => {
                server_edits += 1;
                let before = h.server.version_vector();
                add(&mut h.server, &[&format!("srv{server_edits}")]);
                let update = h.server.export_updates_since(&before).unwrap();
                h.feed(&ServerMessage::Update { from: cid(2), update });
            }
            Op::AckLast => {
                if let Some(seq) = last_seq {
                    h.feed(&ServerMessage::Ack { seq });
                }
            }
            Op::Resolve(replace) => {
                let choice = if replace { TrimmedChoice::ReplaceWithServer } else { TrimmedChoice::KeepMine };
                h.client.resolve_trimmed(h.now, &mut h.store, choice);
                h.drain();
                h.tick();
            }
        }
        if was_stopped && !restarts {
            assert_eq!(h.connects, connects, "a stopped client must not connect");
        }
        assert_eq!(h.client.status().peers, h.client.peers().len());
        for p in h.client.peers().iter() {
            assert!(p.selection.len() <= drafftink_protocol::MAX_SELECTION_IDS);
        }
        h.take_events();
        if let Some((seq, _)) = Harness::updates(&h.frames).last() {
            last_seq = Some(*seq);
        }
        h.take_frames();
    }
    // The document is still usable, and the client can leave cleanly.
    let _ = h.store.version_vector();
    h.client.leave(h.now, &mut h.store);
    h.drain();
}

proptest! {
    #![proptest_config(ProptestConfig::default())]

    #[test]
    fn hostile_sequences_never_panic_and_never_busy_loop(script in ops(blobs())) {
        run(script, false);
    }

    #[test]
    fn hostile_sequences_with_automatic_replacement(script in ops(blobs())) {
        run(script, true);
    }

    #[test]
    fn random_frames_are_refused_without_panic(frames in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..300), 1..20)) {
        let mut h = Harness::new();
        h.start();
        h.event(TransportEvent::Opened);
        for f in frames {
            h.event(TransportEvent::Message(f));
            h.tick();
            if h.client.status().state == ConnState::Reconnecting {
                h.now = h.client.status().retry_at_ms.unwrap();
                h.tick();
                h.event(TransportEvent::Opened);
            }
        }
    }
}
