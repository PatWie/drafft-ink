//! A scripted MCP client over an in-memory pipe, speaking raw JSON-RPC lines to the real server.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use drafftink_engine::Session;
use drafftink_mcp::backend::{Backend, Headless};
use drafftink_mcp::server::DrafftHandler;
use drafftink_mcp::service::Service;
use rmcp::ServiceExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

pub struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: u64,
    pub service: Arc<Service>,
}

/// A headless backend over a session with the real text engine and fake ports.
pub fn headless() -> Backend {
    Backend::Headless(
        Headless::start("test".to_owned(), || drafftink_engine::testing::session_typeset().0).expect("session thread"),
    )
}

pub fn headless_with(make: impl FnOnce() -> Session + Send + 'static) -> Backend {
    Backend::Headless(Headless::start("test".to_owned(), make).expect("session thread"))
}

impl Client {
    /// Starts the server on one end of the pipe and initializes the connection.
    pub async fn start(backend: Backend) -> Self {
        let service = Arc::new(Service::new(backend));
        let (client_end, server_end) = tokio::io::duplex(1 << 20);
        let handler = DrafftHandler::new(service.clone());
        tokio::spawn(async move {
            if let Ok(running) = handler.serve(server_end).await {
                let _ = running.waiting().await;
            }
        });
        let (r, w) = tokio::io::split(client_end);
        let mut c = Self { reader: BufReader::new(r), writer: w, next_id: 1, service };
        let init = c
            .request(
                "initialize",
                json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
            )
            .await;
        assert!(init.get("result").is_some(), "initialize failed: {init}");
        c.notify("notifications/initialized", json!({})).await;
        c
    }

    async fn send(&mut self, v: &Value) {
        let mut line = v.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
    }

    pub async fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params })).await;
    }

    /// Sends a request and returns the whole response object.
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await;
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(std::time::Duration::from_secs(60), self.reader.read_line(&mut line))
                .await
                .expect("server answers")
                .unwrap();
            assert!(n > 0, "the server closed the connection");
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout must carry only JSON-RPC, got {line:?}: {e}"));
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
        }
    }

    pub async fn result(&mut self, method: &str, params: Value) -> Value {
        let r = self.request(method, params).await;
        r.get("result").cloned().unwrap_or_else(|| panic!("expected a result, got {r}"))
    }

    /// Calls a tool and returns the `result` object (`content`, `isError`).
    pub async fn tool(&mut self, name: &str, args: Value) -> Value {
        self.result("tools/call", json!({ "name": name, "arguments": args })).await
    }

    pub async fn tools(&mut self) -> Vec<Value> {
        self.result("tools/list", json!({})).await["tools"].as_array().cloned().expect("tools array")
    }
}

/// The text of a tool result, blocks joined by newlines.
pub fn text_of(result: &Value) -> String {
    result["content"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

pub fn is_error(result: &Value) -> bool {
    result["isError"] == json!(true)
}
