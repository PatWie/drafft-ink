#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_protocol::*;

pub fn cid(n: u8) -> ClientId {
    ClientId::from_bytes([n; 16])
}

pub fn user(n: u8) -> UserInfo {
    UserInfo {
        name: format!("user {n} \u{1f600}"),
        color: 0x6366f1,
    }
}

pub fn full_awareness() -> AwarenessState {
    AwarenessState {
        user: Some(user(1)),
        cursor: Some(Point {
            x: -1234.5,
            y: 99.25,
        }),
        selection: vec!["a1".into(), "b2".into(), "ünï".into()],
        viewport: Some(Viewport {
            x: -10.0,
            y: 20.0,
            width: 1920.0,
            height: 1080.0,
        }),
    }
}

pub fn client_samples() -> Vec<ClientMessage> {
    vec![
        ClientMessage::Join {
            min_version: 1,
            max_version: 1,
            room: RoomId::new("room-1.abc_X").unwrap(),
            client_id: cid(7),
            auth_token: Some("tok.en".into()),
            version: vec![1, 2, 3, 0, 255],
            user: Some(user(2)),
        },
        ClientMessage::Join {
            min_version: 1,
            max_version: 5,
            room: RoomId::new("r").unwrap(),
            client_id: cid(0),
            auth_token: None,
            version: vec![],
            user: None,
        },
        ClientMessage::Update {
            seq: u32::MAX,
            update: vec![0, 1, 2, 3, 250],
        },
        ClientMessage::RequestSync {
            version: vec![9; 300],
        },
        ClientMessage::RequestSync { version: vec![] },
        ClientMessage::Awareness(full_awareness()),
        ClientMessage::Awareness(AwarenessState::default()),
        ClientMessage::Ping { nonce: u64::MAX },
        ClientMessage::Pong { nonce: 0 },
        ClientMessage::Leave,
    ]
}

pub fn server_samples() -> Vec<ServerMessage> {
    vec![
        ServerMessage::Joined {
            protocol_version: 1,
            server: "drafftink-server/0.2.0".into(),
            room: RoomId::new("room").unwrap(),
            client_id: cid(3),
            peers: vec![
                PeerInfo {
                    client_id: cid(4),
                    user: Some(user(4)),
                },
                PeerInfo {
                    client_id: cid(5),
                    user: None,
                },
            ],
            server_version: vec![5, 6, 7],
            limits: Limits::DEFAULT,
        },
        ServerMessage::Joined {
            protocol_version: 1,
            server: String::new(),
            room: RoomId::new("room").unwrap(),
            client_id: cid(3),
            peers: vec![],
            server_version: vec![],
            limits: Limits {
                max_frame_bytes: 1000,
                max_update_bytes: 500,
                max_updates_per_sec: 1,
                max_awareness_per_sec: 2,
            },
        },
        ServerMessage::SyncResponse {
            kind: SyncKind::Updates,
            data: vec![1; 1000],
            server_version: vec![2; 10],
        },
        ServerMessage::SyncResponse {
            kind: SyncKind::Snapshot,
            data: vec![],
            server_version: vec![],
        },
        ServerMessage::Update {
            from: cid(9),
            update: vec![42; 17],
        },
        ServerMessage::Ack { seq: 12345 },
        ServerMessage::PeerJoined {
            peer: PeerInfo {
                client_id: cid(6),
                user: Some(user(6)),
            },
        },
        ServerMessage::PeerLeft { client_id: cid(6) },
        ServerMessage::PeerAwareness {
            from: cid(8),
            state: full_awareness(),
        },
        ServerMessage::Error {
            code: ErrorCode::RateLimited,
            message: "slow down".into(),
            retry_after_ms: Some(1500),
        },
        ServerMessage::Error {
            code: ErrorCode::Internal,
            message: String::new(),
            retry_after_ms: None,
        },
        ServerMessage::Ping { nonce: 77 },
        ServerMessage::Pong { nonce: 78 },
    ]
}

/// Small deterministic generator for the fuzz-style tests (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % (n as u64).max(1)) as usize
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}
