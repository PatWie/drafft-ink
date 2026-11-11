//! MCP over streamable HTTP, as the desktop app serves it: a real MCP client (the `rmcp` client over `reqwest`)
//! against the server with a simulated shell, plus the refusals of the door (token, `Host`, `Origin`, path).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::disallowed_methods)]

mod common;

use std::time::Duration;

use common::{TOKEN, runtime};
use drafftink_bridge::mcp::http::{McpHttpConfig, McpHttpServer};
use drafftink_bridge::shell_sim::McpShellSim;
use drafftink_engine::testing::headless_session;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn config() -> McpHttpConfig {
    McpHttpConfig {
        addr: "127.0.0.1:0".parse().unwrap(),
        token: Some(TOKEN.to_owned()),
        dir: None,
        write_token_file: false,
    }
}

fn start() -> McpShellSim {
    McpShellSim::start(config(), headless_session).expect("server starts")
}

/// One raw HTTP request. Returns the status line and the whole answer.
async fn raw(port: u16, request: &str) -> (String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
    s.write_all(request.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    (text.lines().next().unwrap_or("").to_owned(), text)
}

const BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;

/// A POST with the given extra header lines.
fn post(host: &str, extra: &str) -> String {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\n{extra}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{BODY}",
        BODY.len()
    )
}

fn bearer() -> String {
    format!("Authorization: Bearer {TOKEN}\r\n")
}

#[test]
fn a_request_without_the_token_is_unauthorized_and_says_how_to_authenticate() {
    let shell = start();
    let port = shell.server().port();
    runtime().block_on(async {
        for extra in [String::new(), "Authorization: Bearer nope\r\n".to_owned(), format!("Authorization: {TOKEN}\r\n")]
        {
            let (status, all) = raw(port, &post(&format!("127.0.0.1:{port}"), &extra)).await;
            assert!(status.contains("401"), "{extra:?}: {status}");
            assert!(all.to_ascii_lowercase().contains("www-authenticate: bearer"), "{all}");
        }
    });
}

#[test]
fn the_token_in_the_address_is_not_a_way_in() {
    let shell = start();
    let port = shell.server().port();
    runtime().block_on(async {
        let req = format!("GET /mcp?token={TOKEN} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        let (status, _) = raw(port, &req).await;
        assert!(status.contains("401"), "{status}");
    });
}

#[test]
fn another_host_name_is_forbidden_even_with_the_token() {
    let shell = start();
    let port = shell.server().port();
    runtime().block_on(async {
        for host in ["evil.example", "evil.example:80", &format!("127.0.0.1:{}", port.wrapping_add(1)), "127.0.0.1"] {
            let (status, _) = raw(port, &post(host, &bearer())).await;
            assert!(status.contains("403"), "{host}: {status}");
        }
        let (status, _) = raw(port, &post(&format!("localhost:{port}"), &bearer())).await;
        // The door is open for `localhost`; the MCP layer then asks for `initialize` first (400), not for a token.
        assert!(!status.contains("401") && !status.contains("403"), "localhost is the same machine: {status}");
    });
}

#[test]
fn a_web_page_from_another_origin_is_forbidden_even_with_the_token() {
    let shell = start();
    let port = shell.server().port();
    runtime().block_on(async {
        for origin in ["https://evil.example", "null", "https://drafft.ink", "http://localhost.evil.example"] {
            let extra = format!("{}Origin: {origin}\r\n", bearer());
            let (status, all) = raw(port, &post(&format!("127.0.0.1:{port}"), &extra)).await;
            assert!(status.contains("403"), "{origin}: {status}");
            assert!(!all.to_ascii_lowercase().contains("access-control-allow"), "no CORS header is ever sent: {all}");
        }
    });
}

#[test]
fn a_preflight_gets_no_permission_header() {
    let shell = start();
    let port = shell.server().port();
    runtime().block_on(async {
        let req = format!(
            "OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://localhost:5173\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization\r\nConnection: close\r\n\r\n"
        );
        let (status, all) = raw(port, &req).await;
        assert!(!status.contains("200") && !status.contains("204"), "a preflight is not approved: {status}");
        assert!(!all.to_ascii_lowercase().contains("access-control-allow"), "{all}");
    });
}

#[test]
fn only_the_mcp_path_exists() {
    let shell = start();
    let port = shell.server().port();
    runtime().block_on(async {
        let req = format!("GET /other HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{}Connection: close\r\n\r\n", bearer());
        let (status, _) = raw(port, &req).await;
        assert!(status.contains("404"), "{status}");
    });
}

#[test]
fn it_listens_on_loopback_only_and_refuses_a_weak_token() {
    let (jobs, _host) = drafftink_bridge::host::channel(std::sync::Arc::new(|| {}), None);
    let not_loopback = McpHttpConfig { addr: "0.0.0.0:0".parse().unwrap(), ..config() };
    assert!(matches!(
        McpHttpServer::start(not_loopback, jobs.clone()),
        Err(drafftink_bridge::BridgeError::NotLoopback(_))
    ));
    let weak = McpHttpConfig { token: Some("short".to_owned()), ..config() };
    assert!(matches!(McpHttpServer::start(weak, jobs), Err(drafftink_bridge::BridgeError::Token(_))));
}

#[test]
fn a_random_token_is_64_hex_characters_and_the_file_is_private() {
    let dir = tempfile::tempdir().unwrap();
    let (jobs, _host) = drafftink_bridge::host::channel(std::sync::Arc::new(|| {}), None);
    let cfg = McpHttpConfig { token: None, dir: Some(dir.path().join("mcp-http")), write_token_file: true, ..config() };
    let server = McpHttpServer::start(cfg, jobs).unwrap();
    assert_eq!(server.token().len(), 64);
    assert!(server.token().bytes().all(|b| b.is_ascii_hexdigit()));
    let file = server.token_file().unwrap().to_owned();
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), server.token());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(file.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    server.stop();
    assert!(!file.exists(), "the token file goes with the server");
}

fn tool_args(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn text_of(result: &rmcp::model::CallToolResult) -> String {
    serde_json::to_value(result).unwrap()["content"]
        .as_array()
        .map(|blocks| blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

#[test]
fn a_real_mcp_client_lists_the_tools_builds_a_diagram_and_reads_a_refusal() {
    let shell = start();
    let url = shell.server().url();
    runtime().block_on(async {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url.as_str()).auth_header(TOKEN),
        );
        let client = ().serve(transport).await.expect("initialize");

        let tools = client.list_all_tools().await.expect("tools/list");
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        for want in ["inspect", "diagram", "batch", "commands", "describe", "recent", "status"] {
            assert!(names.contains(&want), "{want} is missing from {names:?}");
        }

        let made = client
            .call_tool(CallToolRequestParams::new("diagram").with_arguments(tool_args(json!({
                "command": "diagram.create",
                "args": {
                    "nodes": [{"id": "a", "label": "Start"}, {"id": "b", "label": "Done"}],
                    "edges": [{"from": "a", "to": "b"}]
                }
            }))))
            .await
            .expect("diagram.create");
        assert_ne!(made.is_error, Some(true), "{}", text_of(&made));

        let outline = client
            .call_tool(
                CallToolRequestParams::new("inspect").with_arguments(tool_args(json!({"command": "doc.outline"}))),
            )
            .await
            .expect("doc.outline");
        let text = text_of(&outline);
        assert!(text.contains("Start") && text.contains("Done"), "{text}");

        let refused = client
            .call_tool(CallToolRequestParams::new("view").with_arguments(tool_args(json!({"command": "view.nope"}))))
            .await
            .expect("a refusal is a result");
        assert_eq!(refused.is_error, Some(true));

        let status = client.call_tool(CallToolRequestParams::new("status")).await.expect("status");
        assert!(text_of(&status).contains("\"mode\":\"desktop\""), "{}", text_of(&status));
        client.cancel().await.ok();
    });
    let state = shell.stop();
    assert!(state.is_object(), "the shell ran the session to the end");
}

#[test]
fn a_client_with_the_wrong_token_cannot_initialize() {
    let shell = start();
    let url = shell.server().url();
    runtime().block_on(async {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url.as_str()).auth_header("0".repeat(64)),
        );
        assert!(().serve(transport).await.is_err());
    });
}
