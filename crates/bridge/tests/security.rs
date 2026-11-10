//! The security properties of the native server: token, deadline, Origin, Host, loopback bind, frame size.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::disallowed_methods)]

mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use common::{TOKEN, config, raw_upgrade, runtime, start};
use drafftink_bridge::client::{BridgeClient, ClientError, ConnectOpts, is_loopback_url};
use drafftink_bridge::rpc::{NOT_AUTHENTICATED, UNAUTHORIZED};
use drafftink_bridge::server::{BridgeConfig, BridgeError, BridgeServer};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

fn url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}/")
}

#[test]
fn the_right_token_is_accepted() {
    let shell = start(config());
    runtime().block_on(async {
        let c = BridgeClient::connect(&url(shell.server().port()), TOKEN).await.expect("connects");
        assert_eq!(c.hello()["protocol"], 1);
        assert_eq!(c.hello()["server"], "drafftink");
    });
}

#[test]
fn a_wrong_token_is_refused_and_the_connection_closes() {
    let shell = start(config());
    runtime().block_on(async {
        for wrong in [TOKEN.replace('0', "1"), TOKEN[..63].to_owned(), String::new(), format!("{TOKEN}x")] {
            let err = match BridgeClient::connect(&url(shell.server().port()), &wrong).await {
                Err(e) => e,
                Ok(_) => panic!("a wrong token must not connect"),
            };
            match err {
                ClientError::Rpc(e) => assert_eq!(e.code, UNAUTHORIZED, "{}", e.message),
                ClientError::Closed => {}
                other => panic!("{other}"),
            }
        }
    });
}

#[test]
fn a_call_before_hello_is_refused_and_the_connection_closes() {
    let shell = start(config());
    runtime().block_on(async {
        let (mut ws, _) = connect_async(url(shell.server().port())).await.expect("upgrade");
        ws.send(Message::text(r#"{"jsonrpc":"2.0","id":1,"method":"execute","params":{"id":"view.get"}}"#))
            .await
            .expect("send");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("no answer") };
        let v: serde_json::Value = serde_json::from_str(t.as_str()).expect("json");
        assert_eq!(v["error"]["code"], NOT_AUTHENTICATED);
        assert!(v["error"]["message"].as_str().unwrap_or("").contains("hello"));
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_))) | None | Some(Err(_))));
    });
}

#[test]
fn silence_after_connecting_is_cut_at_the_auth_deadline() {
    let cfg = BridgeConfig { auth_deadline: Duration::from_millis(400), ..config() };
    let shell = start(cfg);
    runtime().block_on(async {
        let (mut ws, _) = connect_async(url(shell.server().port())).await.expect("upgrade");
        let started = Instant::now();
        let end = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("closed by the server in time");
        assert!(matches!(end, None | Some(Err(_)) | Some(Ok(Message::Close(_)))), "{end:?}");
        assert!(started.elapsed() >= Duration::from_millis(300), "closed too early: {:?}", started.elapsed());
    });
}

#[test]
fn the_default_auth_deadline_is_five_seconds() {
    assert_eq!(BridgeConfig::default().auth_deadline, Duration::from_secs(5));
}

#[test]
fn a_connection_that_never_finishes_the_upgrade_is_cut_too() {
    let cfg = BridgeConfig { auth_deadline: Duration::from_millis(300), ..config() };
    let shell = start(cfg);
    runtime().block_on(async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", shell.server().port())).await.expect("connect");
        s.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n").await.expect("partial request");
        let mut buf = [0u8; 16];
        let n =
            tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf)).await.expect("closed in time").unwrap_or(0);
        assert_eq!(n, 0);
    });
}

#[test]
fn a_foreign_origin_is_refused_before_the_websocket_starts() {
    let shell = start(config());
    runtime().block_on(async {
        let port = shell.server().port();
        for origin in ["https://evil.example", "null", "https://drafft.ink", "http://localhost.evil.com"] {
            let line = raw_upgrade(port, &format!("127.0.0.1:{port}"), Some(origin)).await;
            assert!(line.contains("403"), "{origin}: {line}");
        }
        for origin in [None, Some("http://localhost:5173"), Some("http://127.0.0.1:3030")] {
            let line = raw_upgrade(port, &format!("127.0.0.1:{port}"), origin).await;
            assert!(line.contains("101"), "{origin:?}: {line}");
        }
    });
}

#[test]
fn a_configured_origin_is_allowed() {
    let cfg = BridgeConfig { allowed_origins: vec!["https://app.example".to_owned()], ..config() };
    let shell = start(cfg);
    runtime().block_on(async {
        let port = shell.server().port();
        let line = raw_upgrade(port, &format!("localhost:{port}"), Some("https://app.example")).await;
        assert!(line.contains("101"), "{line}");
    });
}

#[test]
fn a_host_header_that_is_not_this_loopback_port_is_refused() {
    let shell = start(config());
    runtime().block_on(async {
        let port = shell.server().port();
        for host in [
            "evil.example",
            &format!("evil.example:{port}"),
            &format!("127.0.0.1:{}", port.wrapping_add(1)),
            "127.0.0.1",
            "",
        ] {
            let line = raw_upgrade(port, host, None).await;
            assert!(line.contains("403"), "{host:?}: {line}");
        }
        for host in [format!("127.0.0.1:{port}"), format!("localhost:{port}")] {
            let line = raw_upgrade(port, &host, None).await;
            assert!(line.contains("101"), "{host}: {line}");
        }
    });
}

#[test]
fn a_bind_address_that_is_not_127_0_0_1_is_refused() {
    for addr in ["0.0.0.0:0", "192.168.1.5:0", "[::]:0", "[::1]:0", "10.0.0.1:4000"] {
        let cfg = BridgeConfig { addr: addr.parse::<SocketAddr>().expect("addr"), ..config() };
        let r = BridgeServer::start(cfg, std::sync::Arc::new(|| {}));
        assert!(matches!(r, Err(BridgeError::NotLoopback(_))), "{addr}");
    }
}

#[test]
fn a_token_that_is_too_short_is_refused_at_start() {
    let cfg = BridgeConfig { token: Some("short".to_owned()), ..config() };
    let r = BridgeServer::start(cfg, std::sync::Arc::new(|| {}));
    assert!(matches!(r, Err(BridgeError::Token(_))));
}

#[test]
fn a_frame_over_the_limit_closes_the_connection() {
    let cfg = BridgeConfig { max_frame_bytes: 64 * 1024, ..config() };
    let shell = start(cfg);
    runtime().block_on(async {
        let port = shell.server().port();
        let opts = ConnectOpts { origin: None, max_frame_bytes: 8 << 20 };
        let mut c = BridgeClient::connect_with(&url(port), TOKEN, &opts).await.expect("connects");
        let big = json!({ "id": "view.zoom_in", "args": { "pad": "x".repeat(200_000) } });
        let r = c.call("execute", big).await;
        assert!(matches!(r, Err(ClientError::Closed | ClientError::Protocol(_))), "{r:?}");
        // The server is still fine for the next client.
        let mut again = BridgeClient::connect(&url(port), TOKEN).await.expect("connects again");
        assert!(again.call("ping", json!({})).await.is_ok());
    });
}

#[test]
fn the_first_frame_must_be_small() {
    let shell = start(config());
    runtime().block_on(async {
        let (mut ws, _) = connect_async(url(shell.server().port())).await.expect("upgrade");
        let pad = "x".repeat(10_000);
        let hello = json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"token":TOKEN,"pad":pad}});
        ws.send(Message::text(hello.to_string())).await.expect("send");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("no answer") };
        assert!(t.as_str().contains("small"), "{t}");
    });
}

#[test]
fn binary_frames_are_refused() {
    let shell = start(config());
    runtime().block_on(async {
        let port = shell.server().port();
        let (mut ws, _) = connect_async(url(port)).await.expect("upgrade");
        ws.send(Message::Binary(vec![1, 2, 3].into())).await.expect("send");
        let end = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Text(_))) => continue,
                    other => return other,
                }
            }
        })
        .await
        .expect("closed in time");
        assert!(matches!(end, None | Some(Err(_)) | Some(Ok(Message::Close(_)))), "{end:?}");
    });
}

#[test]
fn eight_bad_frames_close_the_connection() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = BridgeClient::connect(&url(shell.server().port()), TOKEN).await.expect("connects");
        let mut closed = false;
        for _ in 0..12 {
            // Not a known method: counted as a bad frame.
            if c.call("no_such_method", json!({})).await.is_err_and(|e| matches!(e, ClientError::Closed)) {
                closed = true;
                break;
            }
        }
        assert!(closed, "the server kept a connection that only sends bad frames");
    });
}

#[test]
fn connections_over_the_limit_are_dropped() {
    let cfg = BridgeConfig { max_connections: 2, ..config() };
    let shell = start(cfg);
    runtime().block_on(async {
        let url = url(shell.server().port());
        let a = BridgeClient::connect(&url, TOKEN).await.expect("first");
        let b = BridgeClient::connect(&url, TOKEN).await.expect("second");
        assert!(BridgeClient::connect(&url, TOKEN).await.is_err(), "the third connection must be dropped");
        drop((a, b));
    });
}

#[test]
fn the_client_will_not_send_a_token_to_a_non_loopback_url() {
    runtime().block_on(async {
        for bad in ["ws://example.com:80/", "ws://192.168.0.2:4000/", "wss://127.0.0.1:4000/"] {
            assert!(!is_loopback_url(bad));
            let r = BridgeClient::connect(bad, TOKEN).await;
            assert!(matches!(r, Err(ClientError::NotLoopback(_))), "{bad}");
        }
    });
}
