//! What only the native shell has: the bridge's door (token, origin of calls) and the registry it carries, plus
//! the screenshot checks that change with commands.

mod common;

use common::{count, list, make_box, num, text, undo};
use drafftink_bridge::client::ClientError;
use drafftink_e2e::{E2eError, close, hex, start_or_skip};
use serde_json::{Value, json};

type Test = Result<(), E2eError>;

#[test]
fn a_wrong_token_is_refused_and_the_right_one_works() -> Test {
    let Some(mut app) = start_or_skip("a_wrong_token_is_refused_and_the_right_one_works")? else { return Ok(()) };
    let wrong = "0".repeat(64);
    match app.connect_with_token(&wrong)? {
        ClientError::Rpc(e) => assert_eq!(e.code, -32001, "a wrong token is error -32001: {}", e.message),
        other => assert!(matches!(other, ClientError::Closed | ClientError::Connect(_)), "unexpected: {other}"),
    }
    assert_eq!(text(&app.exec("engine.state", json!({}))?, "/tool")?, "select", "the right token still works");
    Ok(())
}

#[test]
fn the_registry_of_the_app_is_the_registry_the_browser_specs_are_typed_from() -> Test {
    let Some(mut app) = start_or_skip("the_registry_of_the_app_is_the_registry_the_browser_specs_are_typed_from")?
    else {
        return Ok(());
    };
    let mut live = app.command_ids()?;
    live.sort();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/generated/registry.json");
    let registry: Value =
        serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(|e| E2eError::Shape(e.to_string()))?;
    let want: Vec<String> = list(&registry, "/ids")?.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    // The bridge lists the commands an agent may use; the generated list is the whole registry. Every listed one
    // must be known, and the agent-visible ones must be most of them.
    assert!(
        live.iter().all(|id| want.contains(id)),
        "the app lists a command that the generated types do not know: {:?}",
        live.iter().filter(|id| !want.contains(id)).collect::<Vec<_>>()
    );
    assert!(live.len() * 10 > want.len() * 9, "the app lists {} of {} commands", live.len(), want.len());
    Ok(())
}

#[test]
fn commands_for_the_pointer_are_not_for_agents() -> Test {
    let Some(mut app) = start_or_skip("commands_for_the_pointer_are_not_for_agents")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    let r = app.refusal("gesture.begin", json!({ "command": "element.move", "args": {} }))?;
    assert_eq!(r.code, "not_for_agents");
    assert!(r.message.contains("not available to agents"), "{}", r.message);
    let ring = app.recent(10)?;
    let last = ring.last().ok_or_else(|| E2eError::Shape("empty ring".into()))?;
    assert_eq!(last.get("source").and_then(Value::as_str), Some("agent"));
    Ok(())
}

#[test]
fn the_screen_follows_the_commands() -> Test {
    let Some(mut app) = start_or_skip("the_screen_follows_the_commands")? else { return Ok(()) };
    app.exec("doc.new", json!({}))?;
    let blue = hex("#3366ff").ok_or_else(|| E2eError::Shape("colour".into()))?;
    let id = make_box(&mut app, "ellipse", 100.0, 100.0, 200.0, 120.0, Some("#3366ff"))?;
    app.exec("selection.clear", json!({}))?;
    let region = |app: &mut drafftink_e2e::NativeApp| -> Result<(f64, f64), E2eError> {
        let out = app.exec("view.screenshot", json!({ "region": "all", "scale": 1 }))?;
        let shot = app.screenshot_of(&out)?;
        let corner = shot.at(3, 3).map(|p| close(p, blue, 24)).unwrap_or(false);
        Ok((shot.share((70, 40, 60, 40), blue, 24), f64::from(u8::from(corner))))
    };
    let (centre, corner) = region(&mut app)?;
    assert!(centre > 0.95, "the middle of the ellipse is its fill, share {centre}");
    assert_eq!(corner, 0.0, "the corner of its box is not");

    app.exec("doc.set_background", json!({ "color": "#202020" }))?;
    let out = app.exec("view.screenshot", json!({ "region": "all", "scale": 1, "background": true }))?;
    let dark = app.screenshot_of(&out)?;
    let want = hex("#202020").ok_or_else(|| E2eError::Shape("colour".into()))?;
    assert_eq!(dark.at(3, 3).map(|p| close(p, want, 24)), Some(true), "the background colour is in the corner");
    undo(&mut app, 1)?;

    app.exec("element.delete", json!({ "ids": [id] }))?;
    assert_eq!(count(&mut app)?, 0);
    let nothing = app.refusal("view.screenshot", json!({ "region": "all" }))?;
    assert_eq!(nothing.code, "nothing_to_do", "nothing left to draw: {}", nothing.message);
    undo(&mut app, 1)?;
    let (centre, _) = region(&mut app)?;
    assert!(centre > 0.95, "undo draws it again");
    assert_eq!(num(&app.exec("doc.inspect", json!({}))?, "/elements")?, 1.0);
    Ok(())
}

#[test]
fn an_agent_cannot_switch_on_the_http_door_and_no_http_door_is_open_by_default() -> Test {
    let Some(mut app) = start_or_skip("an_agent_cannot_switch_on_the_http_door_and_no_http_door_is_open_by_default")?
    else {
        return Ok(());
    };
    let r = app.refusal("settings.set", json!({ "key": "agent.mcp_http", "value": true }))?;
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("only be changed by the person at the app"), "{}", r.message);
    let now = app.exec("settings.get", json!({ "key": "agent.mcp_http" }))?;
    assert_eq!(now.pointer("/settings/agent.mcp_http"), Some(&json!(false)), "{now}");
    assert!(!app.bridge_dir().join("mcp-http").exists(), "an app started with --bridge only has no HTTP token file");
    Ok(())
}
