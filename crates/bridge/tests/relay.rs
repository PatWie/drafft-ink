//! The relay and the fake page: pairing, resume, and the checks on the page's connection.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::{raw_upgrade, runtime};
use drafftink_bridge::client::ClientError;
use drafftink_bridge::page_sim::FakePage;
use drafftink_bridge::relay::{NOT_PAIRED, RelayConfig, RelayServer};
use drafftink_bridge::rpc::{TIMED_OUT, UNAUTHORIZED};
use drafftink_bridge::server::BridgeError;
use drafftink_engine::testing::headless_session;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const PAIRING: &str = "pairpairpairpairpairpairpairpairpairpairpairpairpairpairpairpair";

fn relay() -> RelayServer {
    RelayServer::start(RelayConfig { pairing_token: Some(PAIRING.to_owned()), ..RelayConfig::default() })
        .expect("relay starts")
}

#[test]
fn a_call_without_a_page_says_how_to_pair() {
    let relay = relay();
    let e = runtime().block_on(relay.call("ping", json!({}))).unwrap_err();
    assert_eq!(e.code, NOT_PAIRED);
    assert!(e.message.contains("pairing"), "{}", e.message);
    assert!(!relay.status().connected && !relay.status().paired);
}

#[test]
fn the_page_pairs_and_the_relay_drives_its_session() {
    let relay = relay();
    let page = FakePage::connect_hosted(&relay.url(), PAIRING, headless_session).expect("pairs");
    assert!(page.resume_token().is_some_and(|t| t.len() == 64));
    let st = relay.status();
    assert!(st.paired && st.connected);
    assert_eq!(st.page.as_ref().and_then(|p| p.get("agent")), Some(&json!("fake page")));
    runtime().block_on(async {
        let out = relay.call("execute", json!({ "id": "view.zoom_in" })).await.expect("zoom on the page");
        assert_eq!(out["changed"], true);
        let e = relay.call("execute", json!({ "id": "view.zoom_to_fit" })).await.unwrap_err();
        assert!(e.message.contains("`view.center`"), "{}", e.message);
        let recent = relay.call("recent", json!({ "limit": 3 })).await.expect("recent");
        assert_eq!(
            recent["entries"].as_array().and_then(|a| a.first()).map(|e| e["source"].clone()),
            Some(json!("agent"))
        );
    });
    let state = page.disconnect();
    assert!(state["revision"].is_number(), "the page's own session did the work: {state}");
}

#[test]
fn the_pairing_token_works_once_and_the_resume_token_takes_over() {
    let relay = relay();
    let first = FakePage::connect_hosted(&relay.url(), PAIRING, headless_session).expect("pairs");
    let resume = first.resume_token().expect("resume token").to_owned();
    drop(first);
    // The pairing link is spent: a second page with it is refused.
    let again = FakePage::connect_hosted(&relay.url(), PAIRING, headless_session);
    assert!(matches!(again, Err(ClientError::Rpc(ref e)) if e.code == UNAUTHORIZED), "{:?}", again.err());
    // The same tab reconnects with its resume token.
    let back = FakePage::connect_hosted(&relay.url(), &resume, headless_session).expect("resumes");
    assert_eq!(back.resume_token(), Some(resume.as_str()));
    assert!(runtime().block_on(relay.call("ping", json!({}))).is_ok());
}

#[test]
fn a_new_page_replaces_the_old_one() {
    let relay = relay();
    let first = FakePage::connect_hosted(&relay.url(), PAIRING, headless_session).expect("pairs");
    let resume = first.resume_token().expect("resume").to_owned();
    let second = FakePage::connect_hosted(&relay.url(), &resume, headless_session).expect("second tab");
    runtime().block_on(async {
        relay.call("execute", json!({ "id": "tab.new" })).await.expect("new tab");
    });
    // Only the newest page did the work.
    let tabs = |state: serde_json::Value| state["tabs"].as_array().map_or(0, Vec::len);
    assert_eq!(tabs(first.disconnect()), 1);
    assert_eq!(tabs(second.disconnect()), 2);
}

#[test]
fn a_wrong_token_never_pairs() {
    let relay = relay();
    for wrong in ["x".repeat(64), String::new(), PAIRING.replace('p', "q")] {
        let r = FakePage::connect_hosted(&relay.url(), &wrong, headless_session);
        assert!(r.is_err(), "{wrong:?}");
    }
    assert!(!relay.status().paired, "a wrong token must not use up the pairing token");
    assert!(FakePage::connect_hosted(&relay.url(), PAIRING, headless_session).is_ok());
}

#[test]
fn the_page_origin_and_host_are_checked() {
    let relay = relay();
    let port = relay.port();
    runtime().block_on(async {
        for origin in ["https://evil.example", "null", "https://drafft.ink.evil.example"] {
            let line = raw_upgrade(port, &format!("127.0.0.1:{port}"), Some(origin)).await;
            assert!(line.contains("403"), "{origin}: {line}");
        }
        let line = raw_upgrade(port, &format!("rebind.example:{port}"), Some("https://drafft.ink")).await;
        assert!(line.contains("403"), "{line}");
        let line = raw_upgrade(port, &format!("127.0.0.1:{port}"), Some("https://drafft.ink")).await;
        assert!(line.contains("101"), "{line}");
    });
    let evil = FakePage::connect(&relay.url(), PAIRING, Some("https://evil.example"), headless_session);
    assert!(matches!(evil, Err(ClientError::Connect(ref m)) if m.contains("403")), "{:?}", evil.err());
    assert!(!relay.status().paired);
}

#[test]
fn a_relay_only_binds_to_loopback() {
    for addr in ["0.0.0.0:0", "[::1]:0", "192.168.1.2:0"] {
        let cfg = RelayConfig { addr: addr.parse().expect("addr"), ..RelayConfig::default() };
        assert!(matches!(RelayServer::start(cfg), Err(BridgeError::NotLoopback(_))), "{addr}");
    }
}

#[test]
fn the_pairing_link_carries_a_port_and_a_token_in_the_fragment_only() {
    let relay = relay();
    let link = relay.pairing_url("https://drafft.ink/");
    let (base, fragment) = link.split_once('#').expect("fragment");
    assert_eq!(base, "https://drafft.ink/");
    assert_eq!(fragment, format!("drafftink-agent-port={}&drafftink-agent-token={PAIRING}", relay.port()));
    assert_eq!(relay.pairing_url("https://drafft.ink/#old"), link);
}

#[test]
fn a_page_that_disconnects_fails_its_waiting_calls_and_the_status() {
    let relay = relay();
    let page = FakePage::connect_hosted(&relay.url(), PAIRING, headless_session).expect("pairs");
    page.disconnect();
    std::thread::sleep(Duration::from_millis(200));
    assert!(!relay.status().connected);
    let e = runtime().block_on(relay.call("ping", json!({}))).unwrap_err();
    assert_eq!(e.code, NOT_PAIRED);
    assert!(e.message.contains("Reload"), "{}", e.message);
}

#[test]
fn a_page_that_never_answers_times_out_without_pretending_to_cancel() {
    let relay = RelayServer::start(RelayConfig {
        pairing_token: Some(PAIRING.to_owned()),
        request_timeout: Duration::from_millis(300),
        ..RelayConfig::default()
    })
    .expect("relay");
    runtime().block_on(async {
        let mut req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(relay.url())
            .expect("request");
        req.headers_mut().insert("origin", "https://drafft.ink".parse().expect("header"));
        let (mut ws, _) = connect_async(req).await.expect("upgrade");
        ws.send(Message::text(json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"token":PAIRING}}).to_string()))
            .await
            .expect("hello");
        let Some(Ok(Message::Text(_))) = ws.next().await else { panic!("no hello answer") };
        let e = relay.call("execute", json!({ "id": "view.zoom_in" })).await.unwrap_err();
        assert_eq!(e.code, TIMED_OUT);
        assert!(e.message.contains("NOT cancelled"), "{}", e.message);
        // The page did receive the request.
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("no request") };
        assert!(t.as_str().contains("view.zoom_in"));
    });
}
