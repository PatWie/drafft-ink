//! The service token of a private server: `collab.set_access` and `collab.clear_access`, and what the other collab
//! code needs from the [`Secrets`](crate::ports::Secrets) port (`docs/design/collab-e2e.md` section 19.6).
//!
//! The token is never a setting, an output or a ring entry: both values are `secret_args`, [`SecretText`] prints
//! a placeholder, a value that does not fit is refused without repeating it, and `collab.status` shows only whether
//! a token is stored.

use std::fmt;

use drafftink_collab::{AccessCredentials, AccessError, AccessField, AccessGrant, AccessOrigin, ServerUrl};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::address::{AddressError, resolve_server};
use super::{invalid_url, target_url};
use crate::ports::{PortError, Secrets};
use crate::refusal::{Refusal, echo};
use crate::session::Session;

/// A secret value as a command argument. `Debug` and `Serialize` print a placeholder, and a value that is not text
/// is refused without quoting it (serde's own message for a wrong type quotes the value).
#[derive(Clone, PartialEq, Eq, JsonSchema)]
#[schemars(transparent)]
pub struct SecretText(String);

impl SecretText {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText(<hidden>)")
    }
}

impl Serialize for SecretText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<hidden>")
    }
}

impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(text) => Ok(SecretText(text)),
            _ => Err(serde::de::Error::custom("must be text")),
        }
    }
}

/// How a person sees whether this session has a token for the server. Never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessView {
    /// A service token is stored for the server.
    ServiceToken,
    /// This build holds none: the browser carries the sign-in cookie of the page's own server.
    Browser,
    /// No token is stored.
    None,
}

/// What the transports of a new connection to `server` get: the stored token, bound to the origin it was stored for.
/// `None` without a store or without a token for the origin.
pub(super) fn grant_for(secrets: Option<&dyn Secrets>, server: &ServerUrl) -> Result<Option<AccessGrant>, PortError> {
    let Some(secrets) = secrets else { return Ok(None) };
    let origin = AccessOrigin::of(server);
    Ok(secrets.get(&origin)?.map(|credentials| AccessGrant::new(origin, credentials)))
}

impl Session {
    /// Whether a token is stored for the server this session uses (the chosen one, or the setting).
    pub(super) fn access_view(&self) -> AccessView {
        let Some(secrets) = self.ports.secrets.as_deref() else {
            let page = self.ports.net.as_deref().and_then(|n| n.same_origin_url("/ws"));
            return if page.is_some() { AccessView::Browser } else { AccessView::None };
        };
        let Some(origin) = self.server_origin(self.cmd()).ok() else { return AccessView::None };
        match secrets.get(&origin) {
            Ok(Some(_)) => AccessView::ServiceToken,
            _ => AccessView::None,
        }
    }

    /// The origin a command means without a `server` argument: the server this session is connected to or has
    /// chosen, else the one of the setting `collab.server_url`.
    fn server_origin(&self, id: &str) -> Result<AccessOrigin, Refusal> {
        let text = match target_url(self) {
            Some(url) => url,
            None => self.settings.get("collab.server_url").and_then(Value::as_str).unwrap_or_default().to_owned(),
        };
        origin_of(self, id, &text)
    }
}

/// The origin that `text` names: `https://host[:port]`, or a `ws://` or `wss://` address of the server.
fn origin_of(s: &Session, id: &str, text: &str) -> Result<AccessOrigin, Refusal> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return AccessOrigin::parse(trimmed).map_err(|e| invalid_url(id, trimmed, &AddressError::Url(e)));
    }
    let server = resolve_server(s.ports.net.as_deref(), trimmed).map_err(|e| invalid_url(id, trimmed, &e))?;
    Ok(AccessOrigin::of(&server))
}

/// The store of tokens, or `port_unavailable` where there is none (the web app: the browser keeps the sign-in of the
/// page's own server itself).
fn secrets_mut(s: &mut Session) -> Result<&mut (dyn Secrets + 'static), Refusal> {
    let id = s.cmd();
    s.ports.secrets.as_deref_mut().ok_or_else(|| {
        Refusal::port_unavailable(
            id,
            "a store for access tokens",
            "secrets",
            "The web app has none, because the browser keeps the sign-in of the page's server itself. A program without a screen reads DRAFFTINK_ACCESS_CLIENT_ID and DRAFFTINK_ACCESS_CLIENT_SECRET_FILE.",
        )
    })
}

/// The refusal for a value that cannot be a part of a token. The text never holds the value.
fn bad_value(id: &str, e: AccessError) -> Refusal {
    let key = match e {
        AccessError::Empty(f) | AccessError::TooLong(f) | AccessError::NotPlain(f) => match f {
            AccessField::ClientId => "client_id",
            AccessField::ClientSecret => "client_secret",
        },
    };
    Refusal::bad_arg(id, key, e.to_string())
}

fn store_failed(id: &str, verb: &str, e: &PortError) -> Refusal {
    Refusal::custom("io_failed", id, format!("Could not {verb} the access token: {}.", echo(&e.to_string())))
}

// ---- collab.set_access ----

/// Arguments of `collab.set_access`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAccessArgs {
    /// The server the token is for: `https://host[:port]`, or its `wss://` address. Omit it for the server this
    /// session uses (the chosen one, else the setting `collab.server_url`). The token goes to that origin only.
    #[serde(default)]
    pub server: Option<String>,
    /// The client id of the service token (1 to 512 printable ASCII characters).
    pub client_id: SecretText,
    /// The client secret of the service token (1 to 512 printable ASCII characters). It is never shown.
    pub client_secret: SecretText,
}

/// Output of `collab.set_access`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetAccessOut {
    /// False when the same token was stored already.
    pub changed: bool,
    /// The origin the token is stored for.
    pub server: String,
    /// Always `service_token`.
    pub access: AccessView,
}

pub(super) fn set_access(s: &mut Session, a: SetAccessArgs) -> Result<SetAccessOut, Refusal> {
    let id = s.cmd();
    secrets_mut(s)?;
    let credentials =
        AccessCredentials::new(a.client_id.expose(), a.client_secret.expose()).map_err(|e| bad_value(id, e))?;
    let origin = match a.server.as_deref() {
        Some(text) => origin_of(s, id, text)?,
        None => s.server_origin(id)?,
    };
    let secrets = secrets_mut(s)?;
    let changed = secrets.get(&origin).map_err(|e| store_failed(id, "read", &e))?.as_ref() != Some(&credentials);
    if changed {
        secrets.put(&origin, credentials).map_err(|e| store_failed(id, "store", &e))?;
    }
    Ok(SetAccessOut { changed, server: origin.to_string(), access: AccessView::ServiceToken })
}

// ---- collab.clear_access ----

/// Arguments of `collab.clear_access`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ClearAccessArgs {
    /// The server whose token is removed. Omit it for the server this session uses.
    pub server: Option<String>,
}

/// Output of `collab.clear_access`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ClearAccessOut {
    /// Always true.
    pub changed: bool,
    /// The origin whose token was removed.
    pub server: String,
    /// Always `none`.
    pub access: AccessView,
}

pub(super) fn clear_access(s: &mut Session, a: ClearAccessArgs) -> Result<ClearAccessOut, Refusal> {
    let id = s.cmd();
    secrets_mut(s)?;
    let origin = match a.server.as_deref() {
        Some(text) => origin_of(s, id, text)?,
        None => s.server_origin(id)?,
    };
    let secrets = secrets_mut(s)?;
    if !secrets.delete(&origin).map_err(|e| store_failed(id, "remove", &e))? {
        return Err(Refusal::custom(
            "not_found",
            id,
            format!("No access token is stored for {origin}, so there is nothing to remove."),
        ));
    }
    Ok(ClearAccessOut { changed: true, server: origin.to_string(), access: AccessView::None })
}
