//! The desktop app as an MCP server over streamable HTTP (`drafftink --mcp-http`): a real MCP client (the `rmcp`
//! client over `reqwest`) with the bearer token from the private token file builds a flowchart with
//! `diagram.create`, reads it back, looks at the picture the app's renderer draws, and meets the door's refusals.
//! The app runs with no bridge, so HTTP is the only way in.

use drafftink_e2e::{E2eError, Screenshot, start_mcp_http_or_skip};
use http::{HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

type Test = Result<(), E2eError>;

fn runtime() -> Result<tokio::runtime::Runtime, E2eError> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(E2eError::Runtime)
}

fn shape(e: impl std::fmt::Display) -> E2eError {
    E2eError::Shape(e.to_string())
}

fn args(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// The content blocks of a result as JSON.
fn blocks(result: &CallToolResult) -> Result<Vec<Value>, E2eError> {
    let v = serde_json::to_value(result).map_err(shape)?;
    v.get("content").and_then(Value::as_array).cloned().ok_or_else(|| shape(format!("no content in {v}")))
}

fn text_of(result: &CallToolResult) -> Result<String, E2eError> {
    Ok(blocks(result)?.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"))
}

const FLOWCHART: &str = r#"{
  "command": "diagram.create",
  "args": {
    "nodes": [
      {"id": "start", "label": "Start", "shape": "ellipse"},
      {"id": "check", "label": "Valid?", "shape": "diamond"},
      {"id": "ok", "label": "Save"},
      {"id": "bad", "label": "Show error"}
    ],
    "edges": [
      {"from": "start", "to": "check"},
      {"from": "check", "to": "ok", "label": "yes"},
      {"from": "check", "to": "bad", "label": "no"}
    ],
    "layout": {"algorithm": "layered", "direction": "down"}
  }
}"#;

#[test]
fn an_mcp_client_over_http_builds_a_flowchart_in_the_running_app() -> Test {
    let Some(app) = start_mcp_http_or_skip("an_mcp_client_over_http_builds_a_flowchart_in_the_running_app")? else {
        return Ok(());
    };
    // HTTP is the only door: no bridge token file is in the runtime folder.
    assert!(!app.bridge_dir().join("current.port").exists(), "the bridge is off unless asked for");

    runtime()?.block_on(async {
        // The transport spawns its worker, so it is built inside the runtime.
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(app.url()).auth_header(app.token()),
        );
        let client = ().serve(transport).await.map_err(shape)?;

        let tools = client.list_all_tools().await.map_err(shape)?;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        for want in ["inspect", "diagram", "batch", "commands", "describe", "recent", "status"] {
            assert!(names.contains(&want), "{want} is missing from {names:?}");
        }

        let flow: Value = serde_json::from_str(FLOWCHART).map_err(shape)?;
        let made =
            client.call_tool(CallToolRequestParams::new("diagram").with_arguments(args(flow))).await.map_err(shape)?;
        assert_ne!(made.is_error, Some(true), "diagram.create: {}", text_of(&made)?);

        let outline = client
            .call_tool(CallToolRequestParams::new("inspect").with_arguments(args(json!({"command": "doc.outline"}))))
            .await
            .map_err(shape)?;
        let text = text_of(&outline)?;
        for label in ["Start", "Valid?", "Save", "Show error"] {
            assert!(text.contains(label), "the outline lacks `{label}`: {text}");
        }

        // The window draws it: the app's own renderer answers with a picture.
        let shot = client
            .call_tool(CallToolRequestParams::new("inspect").with_arguments(args(json!({
                "command": "view.screenshot", "args": {"region": "all", "scale": 1}
            }))))
            .await
            .map_err(shape)?;
        let image = blocks(&shot)?
            .into_iter()
            .find(|b| b.get("type").and_then(Value::as_str) == Some("image"))
            .ok_or_else(|| shape(format!("no image block in {}", text_of(&shot).unwrap_or_default())))?;
        let data = image.get("data").and_then(Value::as_str).ok_or_else(|| shape("image block without data"))?;
        let picture = Screenshot::from_base64(data)?;
        assert!(picture.width > 100 && picture.height > 100, "{}x{}", picture.width, picture.height);
        let corner = picture.at(2, 2).ok_or_else(|| shape("empty picture"))?;
        let ink = (0..picture.height).step_by(3).flat_map(|y| (0..picture.width).step_by(3).map(move |x| (x, y)));
        assert!(
            ink.filter_map(|(x, y)| picture.at(x, y)).any(|p| p != corner),
            "the picture is one flat colour, so nothing was drawn"
        );

        // It was the agent: the app records the origin of every call.
        let recent = client
            .call_tool(CallToolRequestParams::new("recent").with_arguments(args(json!({"mutating_only": true}))))
            .await
            .map_err(shape)?;
        assert!(text_of(&recent)?.contains("\"source\":\"agent\""), "{}", text_of(&recent)?);

        // One undo step removes the whole diagram.
        let undone = client
            .call_tool(CallToolRequestParams::new("history").with_arguments(args(json!({"command": "history.undo"}))))
            .await
            .map_err(shape)?;
        assert_ne!(undone.is_error, Some(true), "{}", text_of(&undone)?);
        let empty = client
            .call_tool(CallToolRequestParams::new("inspect").with_arguments(args(json!({"command": "doc.outline"}))))
            .await
            .map_err(shape)?;
        assert!(!text_of(&empty)?.contains("Valid?"), "{}", text_of(&empty)?);

        // The tools say where they run.
        let status = client.call_tool(CallToolRequestParams::new("status")).await.map_err(shape)?;
        assert!(text_of(&status)?.contains("\"mode\":\"desktop\""), "{}", text_of(&status)?);
        client.cancel().await.ok();
        Ok::<(), E2eError>(())
    })?;
    Ok(())
}

#[test]
fn the_door_of_the_running_app_wants_the_token_and_a_loopback_origin() -> Test {
    let Some(app) = start_mcp_http_or_skip("the_door_of_the_running_app_wants_the_token_and_a_loopback_origin")? else {
        return Ok(());
    };
    runtime()?.block_on(async {
        let wrong = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(app.url()).auth_header("0".repeat(64)),
        );
        assert!(().serve(wrong).await.is_err(), "a wrong token must not get past initialize");

        // A page of another origin sends `Origin`; the right token does not help it.
        for origin in ["https://evil.example", "null"] {
            let mut headers = std::collections::HashMap::new();
            headers.insert(HeaderName::from_static("origin"), HeaderValue::from_static(origin));
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(app.url())
                    .auth_header(app.token())
                    .custom_headers(headers),
            );
            assert!(().serve(transport).await.is_err(), "a page from `{origin}` must be refused");
        }
        Ok::<(), E2eError>(())
    })?;
    Ok(())
}
