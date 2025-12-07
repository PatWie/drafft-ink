//! WebAssembly entry point and platform-specific code.

use wasm_bindgen::prelude::*;

/// URL parameters for auto-joining a room.
pub struct UrlParams {
    /// Room ID to join
    pub room: Option<String>,
    /// Server host:port (e.g., "localhost:3030")
    pub server: Option<String>,
}

/// Parse URL query parameters for room and server.
/// Supports formats like `?room=abc123&server=localhost:3030`
pub fn get_url_params() -> UrlParams {
    let window = match web_sys::window() {
        Some(w) => w,
        None => {
            return UrlParams {
                room: None,
                server: None,
            };
        }
    };
    let location = window.location();

    let mut room = None;
    let mut server = None;

    // Try query string first (?room=abc123&server=host:port)
    if let Ok(search) = location.search() {
        let params = parse_params(&search);
        if room.is_none() {
            room = params.0;
        }
        if server.is_none() {
            server = params.1;
        }
    }

    // Try hash fragment (#room=abc123&server=host:port)
    if let Ok(hash) = location.hash() {
        let params = parse_params(&hash);
        if room.is_none() {
            room = params.0;
        }
        if server.is_none() {
            server = params.1;
        }
    }

    UrlParams { room, server }
}

/// Parse room and server parameters from a query string or hash.
fn parse_params(s: &str) -> (Option<String>, Option<String>) {
    // Remove leading ? or #
    let s = s.trim_start_matches(|c| c == '?' || c == '#');

    let mut room = None;
    let mut server = None;

    for pair in s.split('&') {
        let mut parts = pair.splitn(2, '=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            if !value.is_empty() {
                match key {
                    "room" => room = Some(value.to_string()),
                    "server" => server = Some(value.to_string()),
                    _ => {}
                }
            }
        }
    }

    (room, server)
}

/// Legacy function for backward compatibility.
pub fn get_room_from_url() -> Option<String> {
    get_url_params().room
}

/// Get the WebSocket server URL.
/// If server is provided in URL params, use that. Otherwise use page origin.
/// Converts http(s) to ws(s) and appends /ws path.
pub fn get_server_url(server_param: Option<&str>) -> Option<String> {
    // If server param provided, construct WebSocket URL from it
    if let Some(server) = server_param {
        // Assume ws:// for explicit server params (user can specify wss:// if needed)
        let server = server.trim();
        if server.starts_with("ws://") || server.starts_with("wss://") {
            // Already has protocol
            if server.ends_with("/ws") {
                return Some(server.to_string());
            } else {
                return Some(format!("{}/ws", server.trim_end_matches('/')));
            }
        } else {
            // Add ws:// protocol
            return Some(format!("ws://{}/ws", server.trim_end_matches('/')));
        }
    }

    // Fall back to page origin
    let window = web_sys::window()?;
    let location = window.location();
    let protocol = location.protocol().ok()?;
    let host = location.host().ok()?;

    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    Some(format!("{}//{}/ws", ws_protocol, host))
}

/// Initialize and run the WASM application.
#[wasm_bindgen(start)]
pub async fn run_wasm() {
    // Set up panic hook for better error messages
    console_error_panic_hook::set_once();

    // Initialize logging
    console_log::init_with_level(log::Level::Info).expect("Failed to initialize logger");

    log::info!("Starting DrafftInk (WASM)");

    // Log URL params if present
    let params = get_url_params();
    if let Some(ref room) = params.room {
        log::info!("Room from URL: {}", room);
    }
    if let Some(ref server) = params.server {
        log::info!("Server from URL: {}", server);
    }

    // Run the app
    crate::App::run().await;
}
