//! Shared helpers of the bridge tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use drafftink_bridge::server::BridgeConfig;
use drafftink_bridge::shell_sim::ShellSim;
use drafftink_engine::testing::headless_session;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A config that writes no token file and uses a fixed token.
pub fn config() -> BridgeConfig {
    BridgeConfig { token: Some(TOKEN.to_owned()), write_token_file: false, ..BridgeConfig::default() }
}

pub fn start(cfg: BridgeConfig) -> ShellSim {
    ShellSim::start(cfg, headless_session).expect("server starts")
}

pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

/// Sends a hand-written upgrade request and returns the first line of the answer.
pub async fn raw_upgrade(port: u16, host: &str, origin: Option<&str>) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
    let origin = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
    let req = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\n{origin}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.expect("write");
    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf)).await.expect("answer in time").expect("read");
    String::from_utf8_lossy(buf.get(..n).unwrap_or_default()).lines().next().unwrap_or("").to_owned()
}
