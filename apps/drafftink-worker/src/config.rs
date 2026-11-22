//! The relay policy of this host, read from the environment variables of the Worker.
//!
//! The shell passes the variables as one JSON object (`{"ROOM_TTL_SECS": "172800", ...}`). Wrangler `vars` are
//! strings or numbers, and `wrangler dev --var KEY:VALUE` always gives strings, so both are accepted. Nothing else
//! configures the relay: the limits announced in `Joined.limits` come from here.

use std::time::Duration;

use drafftink_protocol as proto;
use drafftink_relay::{RelayConfig, Retention};
use serde_json::{Map, Value};

/// A variable that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// The text is not JSON.
    #[error("the settings are not valid JSON: {0}")]
    NotJson(String),
    /// The JSON is not an object of names and values.
    #[error("the settings must be a JSON object")]
    NotAnObject,
    /// A variable has a value of the wrong kind or out of range.
    #[error("{name}: {problem}")]
    Bad {
        /// The variable.
        name: &'static str,
        /// What is wrong, in plain words.
        problem: String,
    },
    /// The values are fine one by one but break a ceiling of the protocol.
    #[error("the limits break a protocol ceiling: {0}")]
    Ceiling(String),
}

fn bad(name: &'static str, problem: impl Into<String>) -> SettingsError {
    SettingsError::Bad { name, problem: problem.into() }
}

/// The values of the variables that the core reads.
struct Vars(Map<String, Value>);

impl Vars {
    /// The raw text of a variable, `None` when it is unset or empty.
    fn text(&self, name: &'static str) -> Result<Option<String>, SettingsError> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.trim().to_owned())),
            Some(Value::Number(n)) => Ok(Some(n.to_string())),
            Some(Value::Bool(b)) => Ok(Some(b.to_string())),
            Some(_) => Err(bad(name, "must be a string or a number")),
        }
    }

    fn number(&self, name: &'static str) -> Result<Option<u64>, SettingsError> {
        self.text(name)?
            .map(|t| t.parse::<u64>().map_err(|_| bad(name, format!("\"{t}\" is not a whole number of at least 0"))))
            .transpose()
    }

    fn at_least_one(&self, name: &'static str) -> Result<Option<u64>, SettingsError> {
        match self.number(name)? {
            Some(0) => Err(bad(name, "must be at least 1")),
            other => Ok(other),
        }
    }

    fn narrow<T: TryFrom<u64>>(&self, name: &'static str) -> Result<Option<T>, SettingsError> {
        self.at_least_one(name)?.map(|n| T::try_from(n).map_err(|_| bad(name, format!("{n} is too large")))).transpose()
    }

    fn flag(&self, name: &'static str) -> Result<Option<bool>, SettingsError> {
        match self.text(name)?.as_deref() {
            None => Ok(None),
            Some("true" | "1") => Ok(Some(true)),
            Some("false" | "0") => Ok(Some(false)),
            Some(other) => Err(bad(name, format!("\"{other}\" is not true or false"))),
        }
    }
}

fn parse(json: &str) -> Result<Vars, SettingsError> {
    match serde_json::from_str::<Value>(json).map_err(|e| SettingsError::NotJson(e.to_string()))? {
        Value::Object(map) => Ok(Vars(map)),
        _ => Err(SettingsError::NotAnObject),
    }
}

/// The policy of a room, from the variables:
///
/// | Variable | Meaning | Default |
/// |---|---|---|
/// | `ROOM_TTL_SECS` | seconds from the last visit to deletion; unset: rooms never expire | unset |
/// | `ROOM_LEAVE_GRACE_SECS` | seconds a room past its deadline lives on after its last connection closed | 60 |
/// | `ALLOW_DELETE_ROOM` | whether a member may delete the room | false |
/// | `MAX_ROOM_STATE_BYTES` | sealed bytes of snapshot and log | 32 MiB |
/// | `MAX_IMAGE_BYTES`, `MAX_ROOM_IMAGE_BYTES`, `MAX_ROOM_IMAGES` | picture limits | protocol ceilings |
/// | `MAX_PEERS_PER_ROOM`, `ROOM_PRESENCE_BUDGET` | people and presence messages per second | 64, 40 |
///
/// Every limit is checked against the ceilings of the protocol.
pub fn relay_config(json: &str) -> Result<RelayConfig, SettingsError> {
    let vars = parse(json)?;
    let grace = vars.number("ROOM_LEAVE_GRACE_SECS")?.map_or(proto::ROOM_LEAVE_GRACE, Duration::from_secs);
    let retention = match vars.at_least_one("ROOM_TTL_SECS")? {
        None => Retention::Never,
        Some(secs) => Retention::After { ttl: Duration::from_secs(secs), grace, recheck: proto::ROOM_RECHECK },
    };
    let mut cfg = RelayConfig { retention, ..RelayConfig::demo() };
    if let Some(allow) = vars.flag("ALLOW_DELETE_ROOM")? {
        cfg.allow_delete_room = allow;
    }
    if let Some(bytes) = vars.at_least_one("MAX_ROOM_STATE_BYTES")? {
        cfg.max_room_state_bytes = bytes;
    }
    if let Some(bytes) = vars.narrow::<u32>("MAX_IMAGE_BYTES")? {
        cfg.limits.max_image_bytes = bytes;
    }
    if let Some(bytes) = vars.at_least_one("MAX_ROOM_IMAGE_BYTES")? {
        cfg.limits.max_room_image_bytes = bytes;
    }
    if let Some(count) = vars.narrow::<u32>("MAX_ROOM_IMAGES")? {
        cfg.limits.max_room_images = count;
    }
    if let Some(peers) = vars.narrow::<usize>("MAX_PEERS_PER_ROOM")? {
        cfg.max_peers = peers;
    }
    if let Some(budget) = vars.narrow::<u16>("ROOM_PRESENCE_BUDGET")? {
        cfg.presence_budget = budget;
    }
    cfg.validate().map_err(|e| SettingsError::Ceiling(e.0))?;
    Ok(cfg)
}

/// The largest sealed picture the edge lets through to a room: the picture limit plus the seal.
pub fn sealed_image_limit(json: &str) -> Result<u64, SettingsError> {
    let cfg = relay_config(json)?;
    Ok(u64::from(cfg.limits.max_image_bytes).saturating_add(proto::SEAL_OVERHEAD as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_variables_give_a_room_that_never_expires_with_the_demo_limits() {
        let cfg = relay_config("{}").unwrap();
        assert_eq!(cfg.retention, Retention::Never);
        assert_eq!(cfg.max_room_state_bytes, proto::MAX_ROOM_STATE_BYTES);
        assert!(!cfg.allow_delete_room);
        assert_eq!(cfg.limits.max_room_images, proto::MAX_ROOM_IMAGES);
    }

    #[test]
    fn strings_and_numbers_both_work_and_set_the_retention() {
        let cfg = relay_config(r#"{"ROOM_TTL_SECS": "172800", "ROOM_LEAVE_GRACE_SECS": 5}"#).unwrap();
        assert_eq!(
            cfg.retention,
            Retention::After {
                ttl: Duration::from_secs(172_800),
                grace: Duration::from_secs(5),
                recheck: proto::ROOM_RECHECK
            }
        );
    }

    #[test]
    fn an_empty_ttl_means_unset() {
        assert_eq!(relay_config(r#"{"ROOM_TTL_SECS": ""}"#).unwrap().retention, Retention::Never);
    }

    #[test]
    fn limits_are_read_and_checked_against_the_ceilings() {
        let cfg = relay_config(
            r#"{"MAX_ROOM_STATE_BYTES":"1000","MAX_ROOM_IMAGES":"7","MAX_ROOM_IMAGE_BYTES":"4096","MAX_IMAGE_BYTES":"2048",
                "MAX_PEERS_PER_ROOM":"8","ROOM_PRESENCE_BUDGET":"20","ALLOW_DELETE_ROOM":"true"}"#,
        )
        .unwrap();
        assert_eq!(cfg.max_room_state_bytes, 1000);
        assert_eq!(
            (cfg.limits.max_room_images, cfg.limits.max_room_image_bytes, cfg.limits.max_image_bytes),
            (7, 4096, 2048)
        );
        assert_eq!((cfg.max_peers, cfg.presence_budget), (8, 20));
        assert!(cfg.allow_delete_room);
        let over = relay_config(r#"{"MAX_ROOM_IMAGES": 201}"#).unwrap_err();
        assert!(matches!(over, SettingsError::Ceiling(_)), "{over}");
        let over = relay_config(r#"{"MAX_IMAGE_BYTES": 4194305}"#).unwrap_err();
        assert!(matches!(over, SettingsError::Ceiling(_)), "{over}");
        assert!(matches!(relay_config(r#"{"MAX_PEERS_PER_ROOM": 257}"#), Err(SettingsError::Ceiling(_))));
    }

    #[test]
    fn nonsense_is_a_typed_error_that_names_the_variable() {
        for (json, name) in [
            (r#"{"ROOM_TTL_SECS": "two days"}"#, "ROOM_TTL_SECS"),
            (r#"{"ROOM_TTL_SECS": "0"}"#, "ROOM_TTL_SECS"),
            (r#"{"ROOM_TTL_SECS": "-5"}"#, "ROOM_TTL_SECS"),
            (r#"{"ROOM_TTL_SECS": 1.5}"#, "ROOM_TTL_SECS"),
            (r#"{"ROOM_TTL_SECS": []}"#, "ROOM_TTL_SECS"),
            (r#"{"ALLOW_DELETE_ROOM": "maybe"}"#, "ALLOW_DELETE_ROOM"),
            (r#"{"MAX_ROOM_STATE_BYTES": "0"}"#, "MAX_ROOM_STATE_BYTES"),
            (r#"{"MAX_ROOM_IMAGES": "99999999999"}"#, "MAX_ROOM_IMAGES"),
        ] {
            match relay_config(json) {
                Err(SettingsError::Bad { name: got, .. }) => assert_eq!(got, name, "{json}"),
                other => panic!("{json}: {other:?}"),
            }
        }
        assert!(matches!(relay_config("not json"), Err(SettingsError::NotJson(_))));
        assert_eq!(relay_config("[1]"), Err(SettingsError::NotAnObject));
    }

    #[test]
    fn the_edge_limit_is_the_picture_limit_plus_the_seal() {
        let limit = sealed_image_limit(r#"{"MAX_IMAGE_BYTES": 1000}"#).unwrap();
        assert_eq!(limit, 1000 + proto::SEAL_OVERHEAD as u64);
        assert_eq!(sealed_image_limit("{}").unwrap(), proto::MAX_SEALED_IMAGE_BYTES as u64);
    }
}
