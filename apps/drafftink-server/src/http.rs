//! The HTTP side: `/ws`, `/health`, the Origin check and the static web client.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::sync::Semaphore;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{debug, warn};

use crate::auth::Auth;
use crate::config::Config;
use crate::room::RoomManager;

/// Which Origin headers may open a WebSocket.
#[derive(Debug, Clone)]
pub struct OriginPolicy {
    allow_all: bool,
    allowed: Vec<String>,
    /// Same-origin requests (Origin authority equals Host) are checked against the loopback
    /// host names when the server listens on loopback, so DNS rebinding cannot reach a local
    /// server.
    loopback_bind: bool,
}

fn normalize(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn host_name(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

fn is_loopback_name(name: &str) -> bool {
    name == "localhost"
        || name.ends_with(".localhost")
        || name == "::1"
        || name.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

impl OriginPolicy {
    /// Builds the policy from the configuration.
    pub fn new(config: &Config) -> Self {
        OriginPolicy {
            allow_all: config.allowed_origins.iter().any(|o| o.trim() == "*"),
            allowed: config.allowed_origins.iter().map(|o| normalize(o)).collect(),
            loopback_bind: config.bind.ip().is_loopback(),
        }
    }

    /// Whether a request with these `Origin` and `Host` headers may upgrade.
    pub fn allows(&self, origin: Option<&str>, host: Option<&str>) -> bool {
        let Some(origin) = origin else { return true }; // native clients send no Origin
        if self.allow_all {
            return true;
        }
        let origin = normalize(origin);
        if self.allowed.contains(&origin) {
            return true;
        }
        let Some(host) = host.map(|h| h.trim().to_ascii_lowercase()) else { return false };
        let authority = origin.split_once("://").map(|(_, rest)| rest);
        if authority != Some(host.as_str()) {
            return false;
        }
        !self.loopback_bind || is_loopback_name(host_name(&host))
    }
}

/// State shared by every request.
pub(crate) struct AppState {
    pub config: Arc<Config>,
    pub manager: Arc<RoomManager>,
    pub auth: Auth,
    pub origin: OriginPolicy,
    pub permits: Arc<Semaphore>,
    pub next_conn: AtomicU64,
}

impl AppState {
    /// A fresh connection id.
    pub(crate) fn new_conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }
}

/// Builds the router.
pub(crate) fn router(state: Arc<AppState>) -> Router {
    let index = state.config.web_dir.join("index.html");
    let web = ServeDir::new(&state.config.web_dir)
        .precompressed_br()
        .precompressed_gzip()
        .append_index_html_on_directories(true)
        .not_found_service(ServeFile::new(index));
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health))
        .fallback_service(web)
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn security_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    res
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    let rooms = state.manager.loaded_rooms().await;
    let connections = state.config.limits.max_connections.saturating_sub(state.permits.available_permits());
    let body = format!(
        "{{\"status\":\"ok\",\"version\":\"{}\",\"rooms\":{rooms},\"connections\":{connections}}}",
        env!("CARGO_PKG_VERSION")
    );
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn ws_handler(State(state): State<Arc<AppState>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let origin = headers.get(header::ORIGIN);
    // A non-UTF-8 Origin cannot be one we allow.
    let origin_text = origin.map(|v| v.to_str().unwrap_or("\u{fffd}"));
    if !state.origin.allows(origin_text, header_str(&headers, header::HOST)) {
        warn!("refused a WebSocket from a disallowed origin");
        return (StatusCode::FORBIDDEN, "this origin is not allowed to connect; see --allowed-origin").into_response();
    }
    let Ok(permit) = state.permits.clone().try_acquire_owned() else {
        warn!("connection limit of {} reached", state.config.limits.max_connections);
        return (StatusCode::SERVICE_UNAVAILABLE, "the server has reached its connection limit").into_response();
    };
    let max = state.config.limits.max_frame_bytes;
    ws.max_message_size(max).max_frame_size(max).on_upgrade(move |socket| async move {
        debug!("connection opened");
        crate::conn::serve(socket, state, permit).await;
        debug!("connection closed");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(origins: &[&str], bind: &str) -> OriginPolicy {
        let config = Config {
            bind: bind.parse().expect("addr"),
            allowed_origins: origins.iter().map(|s| (*s).to_owned()).collect(),
            ..Config::default()
        };
        OriginPolicy::new(&config)
    }

    #[test]
    fn no_origin_is_allowed() {
        assert!(policy(&[], "127.0.0.1:3030").allows(None, Some("localhost:3030")));
    }

    #[test]
    fn same_origin_on_loopback_needs_a_loopback_host() {
        let p = policy(&[], "127.0.0.1:3030");
        assert!(p.allows(Some("http://localhost:3030"), Some("localhost:3030")));
        assert!(p.allows(Some("http://127.0.0.1:3030"), Some("127.0.0.1:3030")));
        assert!(p.allows(Some("http://[::1]:3030"), Some("[::1]:3030")));
        // DNS rebinding: the page's own origin matches Host, but the name is not loopback.
        assert!(!p.allows(Some("http://evil.example:3030"), Some("evil.example:3030")));
    }

    #[test]
    fn cross_origin_is_refused_unless_listed() {
        let p = policy(&[], "127.0.0.1:3030");
        assert!(!p.allows(Some("https://evil.example"), Some("localhost:3030")));
        assert!(!p.allows(Some("null"), Some("localhost:3030")));
        let p = policy(&["https://Draw.Example/"], "127.0.0.1:3030");
        assert!(p.allows(Some("https://draw.example"), Some("localhost:3030")));
        assert!(!p.allows(Some("http://draw.example"), Some("localhost:3030")));
        assert!(!p.allows(Some("https://draw.example:8443"), Some("localhost:3030")));
    }

    #[test]
    fn same_origin_on_a_public_bind_matches_host() {
        let p = policy(&[], "0.0.0.0:3030");
        assert!(p.allows(Some("https://draw.example.com"), Some("draw.example.com")));
        assert!(!p.allows(Some("https://other.example.com"), Some("draw.example.com")));
        assert!(!p.allows(Some("https://draw.example.com"), None));
    }

    #[test]
    fn star_allows_everything() {
        let p = policy(&["*"], "127.0.0.1:3030");
        assert!(p.allows(Some("https://evil.example"), Some("localhost:3030")));
        assert!(p.allows(Some("null"), None));
    }
}
