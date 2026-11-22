#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `NativeBlobTransport` (the picture client of `drafftink-collab`) against the real server's image routes, and
//! against two small servers that misbehave.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::test_config;
use drafftink_collab::{
    BlobAuth, BlobOp, BlobReply, BlobRequest, BlobTransport, BlobTransportError, MAX_BLOB_BODY_BYTES,
    NativeBlobTransport,
};
use drafftink_protocol::v2::{ClientMessage, JoinMode, Proof, ServerMessage};
use drafftink_protocol::{ClientId, MAX_IMAGE_BYTES, SEAL_OVERHEAD};
use drafftink_seal::{RoomKey, RoomKeys};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::protocol::Message;

const PATIENCE: Duration = Duration::from_secs(60);

/// A transport and the signal its wake hook sets.
fn transport() -> (NativeBlobTransport, Arc<Notify>, Arc<AtomicUsize>) {
    let notify = Arc::new(Notify::new());
    let wakes = Arc::new(AtomicUsize::new(0));
    let (n, w) = (Arc::clone(&notify), Arc::clone(&wakes));
    let hook = Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
        n.notify_one();
    });
    (NativeBlobTransport::new(hook), notify, wakes)
}

/// Sends one request and waits for its answer, woken by the hook.
async fn call(
    t: &mut NativeBlobTransport,
    notify: &Notify,
    id: u64,
    url: String,
    op: BlobOp,
    auth: &BlobAuth,
) -> Result<BlobReply, String> {
    t.send(BlobRequest { id, url, op, auth: auth.clone() }).expect("accepted");
    let wait = async {
        loop {
            if let Some(response) = t.poll() {
                assert_eq!(response.id, id);
                return response.result;
            }
            notify.notified().await;
        }
    };
    tokio::time::timeout(PATIENCE, wait).await.expect("an answer in time")
}

/// Makes the room on the server: a sealed room exists after its first `Join` with `Create`.
async fn make_room(addr: SocketAddr, keys: &RoomKeys) -> tokio_tungstenite::WebSocketStream<impl Sized> {
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/r/{}/ws", keys.room_id())).await.expect("connect");
    let join = ClientMessage::Join {
        min_version: 2,
        max_version: 2,
        room: keys.room_id(),
        client_id: ClientId::from_bytes([1; 16]),
        mode: JoinMode::Create,
        proof: Proof::token(keys.membership_token()),
        last_seq: 0,
        captcha: None,
    };
    ws.send(Message::Binary(join.encode().expect("encodes").into())).await.expect("sends");
    loop {
        let Some(Ok(Message::Binary(frame))) = ws.next().await else { panic!("the server closed") };
        if matches!(ServerMessage::decode(&frame), Ok(ServerMessage::Joined { .. })) {
            return ws;
        }
    }
}

#[tokio::test]
async fn put_get_head_and_list_work_against_the_real_server() {
    let server = drafftink_server::start(test_config()).await.expect("starts");
    let addr = server.local_addr();
    let keys = RoomKeys::new(&RoomKey::from_bytes([42; 32]));
    let _member = make_room(addr, &keys).await;
    let (mut t, notify, wakes) = transport();
    let auth = BlobAuth::member(keys.membership_token());
    let base = format!("http://{addr}/r/{}/f", keys.room_id());

    // The biggest picture the room takes.
    let picture = vec![0x5a; MAX_IMAGE_BYTES];
    let name = keys.image_name(&[7; 32]);
    let sealed = keys.seal_image(&name, &picture).expect("seals").into_bytes();
    assert_eq!(sealed.len(), MAX_IMAGE_BYTES + SEAL_OVERHEAD);
    let url = format!("{base}/{name}");

    let put = call(&mut t, &notify, 1, url.clone(), BlobOp::Put(sealed.clone()), &auth).await.unwrap();
    assert_eq!(put.status, 201, "{}", String::from_utf8_lossy(&put.body));
    let totals: serde_json::Value = serde_json::from_slice(&put.body).unwrap();
    assert_eq!(totals["room_images"], 1);
    assert_eq!(totals["room_image_bytes"], sealed.len());

    let got = call(&mut t, &notify, 2, url.clone(), BlobOp::Get, &auth).await.unwrap();
    assert_eq!((got.status, got.body.len()), (200, sealed.len()));
    assert_eq!(got.body, sealed);
    assert_eq!(got.content_length, Some(sealed.len() as u64));

    let head = call(&mut t, &notify, 3, url.clone(), BlobOp::Head, &auth).await.unwrap();
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.content_length, Some(sealed.len() as u64), "HEAD says how long the sealed picture is");

    let list = call(&mut t, &notify, 4, base.clone(), BlobOp::List, &auth).await.unwrap();
    assert_eq!(list.status, 200);
    let list: serde_json::Value = serde_json::from_slice(&list.body).unwrap();
    assert_eq!(list[0]["name"], name.to_string());
    assert_eq!(list[0]["size"], sealed.len());

    let other = keys.image_name(&[8; 32]);
    let missing_url = format!("{base}/{other}");
    for (id, op) in [(5, BlobOp::Get), (6, BlobOp::Head)] {
        let missing = call(&mut t, &notify, id, missing_url.clone(), op, &auth).await.unwrap();
        assert_eq!(missing.status, 404);
    }

    // One byte more than a sealed picture can be.
    let too_big = vec![0u8; MAX_IMAGE_BYTES + SEAL_OVERHEAD + 1];
    let refused = call(&mut t, &notify, 7, format!("{base}/{other}"), BlobOp::Put(too_big), &auth).await;
    assert!(refused.as_ref().map_or(true, |r| r.status == 413), "{refused:?}");

    assert!(wakes.load(Ordering::SeqCst) >= 7, "the hook runs for every answer");
    server.shutdown().await;
}

#[tokio::test]
async fn a_token_of_another_room_is_a_401_and_no_bytes() {
    let server = drafftink_server::start(test_config()).await.expect("starts");
    let addr = server.local_addr();
    let keys = RoomKeys::new(&RoomKey::from_bytes([42; 32]));
    let _member = make_room(addr, &keys).await;
    let (mut t, notify, _) = transport();
    let member = BlobAuth::member(keys.membership_token());
    let stranger = BlobAuth::member(RoomKeys::new(&RoomKey::from_bytes([43; 32])).membership_token());
    let base = format!("http://{addr}/r/{}/f", keys.room_id());
    let name = keys.image_name(&[7; 32]);
    let sealed = keys.seal_image(&name, b"a picture").expect("seals").into_bytes();
    let url = format!("{base}/{name}");
    assert_eq!(call(&mut t, &notify, 1, url.clone(), BlobOp::Put(sealed), &member).await.unwrap().status, 201);

    for (id, url, op) in
        [(2, url.clone(), BlobOp::Get), (3, url.clone(), BlobOp::Head), (4, base.clone(), BlobOp::List)]
    {
        let reply = call(&mut t, &notify, id, url, op, &stranger).await.unwrap();
        assert_eq!(reply.status, 401);
        assert!(reply.body.len() < 300, "an error text, not a picture or a list");
    }
    let put = call(&mut t, &notify, 5, url, BlobOp::Put(vec![1; 200]), &stranger).await.unwrap();
    assert_eq!(put.status, 401);
    server.shutdown().await;
}

#[tokio::test]
async fn plain_http_to_another_machine_is_refused_before_any_connection() {
    let (mut t, _, wakes) = transport();
    let auth = BlobAuth::member(RoomKeys::new(&RoomKey::from_bytes([1; 32])).membership_token());
    for url in ["http://192.0.2.1/r/x/f", "http://example.com/r/x/f", "ftp://localhost/x", "file:///etc/passwd", "no"] {
        let request = BlobRequest { id: 1, url: url.to_owned(), op: BlobOp::List, auth: auth.clone() };
        assert!(matches!(t.send(request), Err(BlobTransportError::Refused(_))), "{url}");
    }
    assert!(t.poll().is_none());
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
}

/// A server that answers every connection with `answer`, written by `respond`.
async fn rogue(
    respond: impl Fn(&mut tokio::net::TcpStream) -> futures_util::future::BoxFuture<'_, ()> + Send + Sync + 'static,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let respond = Arc::new(respond);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let respond = Arc::clone(&respond);
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                respond(&mut stream).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn an_answer_larger_than_any_picture_is_a_failure_not_memory() {
    let addr = rogue(|stream| {
        Box::pin(async move {
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_BLOB_BODY_BYTES * 4);
            let _ = stream.write_all(head.as_bytes()).await;
            let chunk = vec![0u8; 64 * 1024];
            for _ in 0..(MAX_BLOB_BODY_BYTES * 4 / chunk.len()) {
                if stream.write_all(&chunk).await.is_err() {
                    return;
                }
            }
        })
    })
    .await;
    let (mut t, notify, _) = transport();
    let auth = BlobAuth::member(RoomKeys::new(&RoomKey::from_bytes([1; 32])).membership_token());
    let result = call(&mut t, &notify, 1, format!("http://{addr}/r/x/f/y"), BlobOp::Get, &auth).await;
    assert!(result.is_err(), "{:?}", result.map(|r| r.body.len()));
}

#[tokio::test]
async fn a_redirect_is_not_followed_and_a_retry_after_is_read() {
    let addr = rogue(|stream| {
        Box::pin(async move {
            let head = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nRetry-After: 9\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes()).await;
        })
    })
    .await;
    let (mut t, notify, _) = transport();
    let auth = BlobAuth::member(RoomKeys::new(&RoomKey::from_bytes([1; 32])).membership_token());
    let reply = call(&mut t, &notify, 1, format!("http://{addr}/r/x/f/y"), BlobOp::Get, &auth).await.unwrap();
    assert_eq!(reply.status, 302, "the Authorization header must not follow a redirect");
    assert_eq!(reply.retry_after_ms, Some(9_000));
}

#[tokio::test]
async fn a_connection_that_fails_is_an_error_text_without_the_token() {
    // A port nobody listens on.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (mut t, notify, _) = transport();
    let keys = RoomKeys::new(&RoomKey::from_bytes([1; 32]));
    let auth = BlobAuth::member(keys.membership_token());
    let result = call(&mut t, &notify, 1, format!("http://{addr}/r/x/f"), BlobOp::List, &auth).await;
    let text = result.unwrap_err();
    assert!(!text.contains(auth.header_value()), "{text}");
}

#[tokio::test]
async fn five_requests_run_side_by_side() {
    // A server that answers only when five requests are waiting shows that the transport has them in flight together.
    let waiting = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&waiting);
    let addr = rogue(move |stream| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            seen.fetch_add(1, Ordering::SeqCst);
            while seen.load(Ordering::SeqCst) < 5 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
        })
    })
    .await;
    let (mut t, notify, _) = transport();
    let auth = BlobAuth::member(RoomKeys::new(&RoomKey::from_bytes([1; 32])).membership_token());
    for id in 1..=5 {
        t.send(BlobRequest { id, url: format!("http://{addr}/r/x/f/{id}"), op: BlobOp::Get, auth: auth.clone() })
            .expect("accepted");
    }
    let mut answered = Vec::new();
    let wait = async {
        while answered.len() < 5 {
            match t.poll() {
                Some(response) => answered.push(response.id),
                None => notify.notified().await,
            }
        }
    };
    tokio::time::timeout(PATIENCE, wait).await.expect("all five answered, so all five were in flight together");
    answered.sort_unstable();
    assert_eq!(answered, [1, 2, 3, 4, 5]);
}
