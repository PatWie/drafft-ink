//! Server configuration: the plain [`Config`] the server runs with, and the command line and
//! environment front end ([`Cli`]) that builds it.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use drafftink_protocol as proto;

use crate::store::StorageConfig;

/// The project repository, from the package metadata (`repository` in the workspace `Cargo.toml`).
pub const DEFAULT_SOURCE_URL: &str = env!("CARGO_PKG_REPOSITORY");

/// Longest source URL accepted. It goes into a response header.
const MAX_SOURCE_URL_BYTES: usize = 2048;

/// Size and rate limits. Every value is at most the hard ceiling in `drafftink-protocol`.
#[derive(Debug, Clone)]
pub struct ServerLimits {
    /// Largest WebSocket message accepted.
    pub max_frame_bytes: usize,
    /// Largest `Update` payload accepted.
    pub max_update_bytes: usize,
    /// `Update` messages per second per connection (the bucket allows twice this as a burst).
    pub max_updates_per_sec: u16,
    /// `Awareness` messages per second per connection.
    pub max_awareness_per_sec: u16,
    /// Loaded rooms at once.
    pub max_rooms: usize,
    /// Peers in one room.
    pub max_peers_per_room: usize,
    /// Open WebSocket connections at once.
    pub max_connections: usize,
    /// Stored bytes (snapshot plus log) a room may grow to; further updates get `TooLarge`.
    pub max_room_bytes: u64,
    /// Messages queued for one slow connection before it is told to resync.
    pub outbox_messages: usize,
    /// Bytes queued for one slow connection before it is told to resync.
    pub outbox_bytes: usize,
    /// Rate-limit breaches in a row that close the connection.
    pub max_breaches: u32,
}

impl Default for ServerLimits {
    fn default() -> Self {
        ServerLimits {
            max_frame_bytes: proto::MAX_FRAME_BYTES,
            max_update_bytes: proto::MAX_UPDATE_BYTES,
            max_updates_per_sec: proto::DEFAULT_MAX_UPDATES_PER_SEC,
            max_awareness_per_sec: proto::DEFAULT_MAX_AWARENESS_PER_SEC,
            max_rooms: 1000,
            max_peers_per_room: 64,
            max_connections: 4096,
            max_room_bytes: 256 * 1024 * 1024,
            outbox_messages: 256,
            outbox_bytes: 32 * 1024 * 1024,
            max_breaches: 50,
        }
    }
}

impl ServerLimits {
    /// What `Joined.limits` announces.
    pub fn announced(&self) -> proto::Limits {
        proto::Limits {
            max_frame_bytes: u32::try_from(self.max_frame_bytes).unwrap_or(u32::MAX),
            max_update_bytes: u32::try_from(self.max_update_bytes).unwrap_or(u32::MAX),
            max_updates_per_sec: self.max_updates_per_sec,
            max_awareness_per_sec: self.max_awareness_per_sec,
        }
    }
}

/// Timers. Tests shorten them.
#[derive(Debug, Clone)]
pub struct Timing {
    /// A connection that sends no `Join` in this time is closed.
    pub join_timeout: Duration,
    /// Updates are written to the store at most this often; zero writes before every `Ack`.
    pub flush_interval: Duration,
    /// A room without peers is flushed, compacted and unloaded after this time.
    pub room_idle: Duration,
    /// Log entries that trigger a compaction.
    pub compact_entries: u64,
    /// Log bytes that trigger a compaction.
    pub compact_bytes: u64,
    /// Rooms untouched for this long are deleted from the store. `None` keeps them for ever.
    pub expire_after: Option<Duration>,
    /// Interval between server pings.
    pub ping_interval: Duration,
    /// A connection with no frame for this long is closed.
    pub idle_timeout: Duration,
    /// A single store call that takes longer than this fails.
    pub store_timeout: Duration,
    /// Wait before a failed store write is tried again.
    pub flush_retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            join_timeout: Duration::from_secs(10),
            flush_interval: Duration::from_secs(2),
            room_idle: Duration::from_secs(600),
            compact_entries: 1000,
            compact_bytes: 8 * 1024 * 1024,
            expire_after: None,
            ping_interval: Duration::from_millis(u64::from(proto::PING_INTERVAL_MS)),
            idle_timeout: Duration::from_millis(u64::from(proto::IDLE_TIMEOUT_MS)),
            store_timeout: Duration::from_secs(30),
            flush_retry: Duration::from_secs(1),
        }
    }
}

/// Everything the server needs to run.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to listen on.
    pub bind: SocketAddr,
    /// Directory with the built web client.
    pub web_dir: PathBuf,
    /// Room storage.
    pub storage: StorageConfig,
    /// Origins allowed to open a WebSocket besides the server's own (`*` allows every origin).
    pub allowed_origins: Vec<String>,
    /// Accepted bearer tokens. Empty means auth is off.
    pub auth_tokens: Vec<String>,
    /// Accept the JSON text form on `/ws`.
    pub debug_json: bool,
    /// Where the corresponding source of this server and its web client can be had: the AGPL-3.0 section 13 offer to
    /// everyone who uses the app over the network. Served at `/source`, `/source.json` and in a `Link` header, and
    /// shown in the web app's About dialog. A host that runs a modified version points this at its own source.
    pub source_url: String,
    /// Limits.
    pub limits: ServerLimits,
    /// Timers.
    pub timing: Timing,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: SocketAddr::from(([127, 0, 0, 1], 3030)),
            web_dir: PathBuf::from("web-dist"),
            storage: StorageConfig::default_filesystem(),
            allowed_origins: Vec::new(),
            auth_tokens: Vec::new(),
            debug_json: false,
            source_url: DEFAULT_SOURCE_URL.to_owned(),
            limits: ServerLimits::default(),
            timing: Timing::default(),
        }
    }
}

/// A configuration the server refuses to start with.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

fn bad(message: impl Into<String>) -> ConfigError {
    ConfigError(message.into())
}

impl Config {
    /// Checks the values against each other and against the protocol ceilings.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let l = &self.limits;
        if l.max_update_bytes == 0 || l.max_update_bytes > proto::MAX_UPDATE_BYTES {
            return Err(bad(format!("max-update-bytes must be between 1 and {}", proto::MAX_UPDATE_BYTES)));
        }
        if l.max_frame_bytes > proto::MAX_FRAME_BYTES || l.max_frame_bytes < l.max_update_bytes + 1024 {
            return Err(bad(format!(
                "max-frame-bytes must be between max-update-bytes + 1024 and {}",
                proto::MAX_FRAME_BYTES
            )));
        }
        if l.max_updates_per_sec == 0 || l.max_awareness_per_sec == 0 {
            return Err(bad("max-updates-per-sec and max-awareness-per-sec must be at least 1"));
        }
        if l.max_peers_per_room == 0 || l.max_peers_per_room > proto::MAX_PEERS {
            return Err(bad(format!("max-peers-per-room must be between 1 and {}", proto::MAX_PEERS)));
        }
        if l.max_rooms == 0 || l.max_connections == 0 {
            return Err(bad("max-rooms and max-connections must be at least 1"));
        }
        if !is_source_url(&self.source_url) {
            return Err(bad(format!(
                "source-url must be an http:// or https:// address of at most {MAX_SOURCE_URL_BYTES} bytes without spaces or control characters, got {:?}",
                self.source_url.chars().take(80).collect::<String>()
            )));
        }
        if self.auth_tokens.iter().any(|t| t.is_empty() || t.len() > proto::MAX_AUTH_TOKEN_BYTES) {
            return Err(bad(format!("auth tokens must be 1 to {} bytes", proto::MAX_AUTH_TOKEN_BYTES)));
        }
        Ok(())
    }
}

/// True for an `http://` or `https://` address that can travel in a header and a redirect.
fn is_source_url(url: &str) -> bool {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    url.len() <= MAX_SOURCE_URL_BYTES
        && rest.is_some_and(|r| !r.is_empty())
        && url.bytes().all(|b| b.is_ascii_graphic())
}

/// The storage provider named on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StorageKind {
    /// Files under `--data-dir` (default).
    #[value(alias = "fs")]
    Filesystem,
    /// Process memory; lost on restart.
    Memory,
    /// A SQLite database (build with `--features sqlite`).
    Sqlite,
    /// An S3-compatible bucket (build with `--features s3`).
    S3,
}

/// Command line and environment. Every flag has an environment variable of the same name with
/// the `DRAFFTINK_` prefix, so a container needs no arguments.
#[derive(Debug, Parser)]
#[command(name = "drafftink-server", version, about = "Collaboration server for drafft.ink")]
pub struct Cli {
    /// Address to listen on. The default is loopback only; pass an explicit address such as
    /// 0.0.0.0:3030 to accept connections from other machines.
    #[arg(long, env = "DRAFFTINK_BIND", default_value = "127.0.0.1:3030")]
    pub bind: SocketAddr,

    /// Directory with the built web client, served at `/`.
    #[arg(long, env = "DRAFFTINK_WEB_DIR", default_value = "web-dist")]
    pub web_dir: PathBuf,

    /// Where rooms are stored.
    #[arg(long, env = "DRAFFTINK_STORAGE", value_enum, default_value = "filesystem")]
    pub storage: StorageKind,

    /// Root directory of the filesystem store.
    #[arg(long, env = "DRAFFTINK_DATA_DIR", default_value = "data")]
    pub data_dir: PathBuf,

    /// SQLite database file (default: `<data-dir>/drafftink.sqlite3`).
    #[arg(long, env = "DRAFFTINK_SQLITE_PATH")]
    pub sqlite_path: Option<PathBuf>,

    /// S3 bucket name.
    #[arg(long, env = "DRAFFTINK_S3_BUCKET")]
    pub s3_bucket: Option<String>,

    /// S3 endpoint URL for S3-compatible services such as MinIO (default: AWS).
    #[arg(long, env = "DRAFFTINK_S3_ENDPOINT")]
    pub s3_endpoint: Option<String>,

    /// S3 region.
    #[arg(long, env = "DRAFFTINK_S3_REGION")]
    pub s3_region: Option<String>,

    /// S3 access key id (default: the usual AWS credential sources).
    #[arg(long, env = "AWS_ACCESS_KEY_ID")]
    pub s3_access_key_id: Option<String>,

    /// S3 secret access key. Prefer the environment variable over the flag.
    #[arg(long, env = "AWS_SECRET_ACCESS_KEY", hide_env_values = true)]
    pub s3_secret_access_key: Option<String>,

    /// S3 session token for temporary credentials.
    #[arg(long, env = "AWS_SESSION_TOKEN", hide_env_values = true)]
    pub s3_session_token: Option<String>,

    /// Key prefix inside the bucket.
    #[arg(long, env = "DRAFFTINK_S3_PREFIX")]
    pub s3_prefix: Option<String>,

    /// Allow a plain http:// S3 endpoint (for local MinIO).
    #[arg(long, env = "DRAFFTINK_S3_ALLOW_HTTP")]
    pub s3_allow_http: bool,

    /// Use virtual-hosted-style S3 addressing instead of path style.
    #[arg(long, env = "DRAFFTINK_S3_VIRTUAL_HOSTED_STYLE")]
    pub s3_virtual_hosted_style: bool,

    /// Delete S3 objects one request at a time, for services without DeleteObjects.
    #[arg(long, env = "DRAFFTINK_S3_DISABLE_BULK_DELETE")]
    pub s3_disable_bulk_delete: bool,

    /// Origin allowed to open a WebSocket, such as `https://draw.example.com`. Repeat the flag
    /// or separate values with commas. The server's own origin is always allowed; `*` allows
    /// every origin.
    #[arg(long = "allowed-origin", env = "DRAFFTINK_ALLOWED_ORIGINS", value_delimiter = ',')]
    pub allowed_origins: Vec<String>,

    /// Bearer token that clients must send in `Join`. Giving any token turns auth on. Repeat
    /// the flag or separate values with commas. Prefer the environment variable or
    /// `--auth-token-file`, because command lines are visible to other users.
    #[arg(long = "auth-token", env = "DRAFFTINK_AUTH_TOKENS", value_delimiter = ',', hide_env_values = true)]
    pub auth_tokens: Vec<String>,

    /// File with one accepted token per line. Giving it turns auth on.
    #[arg(long, env = "DRAFFTINK_AUTH_TOKEN_FILE")]
    pub auth_token_file: Option<PathBuf>,

    /// Where users of this server get its source code (AGPL-3.0 section 13). Point it at your own repository or
    /// archive when you run a modified version. Default: the project repository.
    #[arg(long, env = "DRAFFTINK_SOURCE_URL", default_value = DEFAULT_SOURCE_URL)]
    pub source_url: String,

    /// Accept the JSON text form of the protocol on /ws, for debugging.
    #[arg(long, env = "DRAFFTINK_DEBUG_JSON")]
    pub debug_json: bool,

    /// Most loaded rooms.
    #[arg(long, env = "DRAFFTINK_MAX_ROOMS", default_value_t = ServerLimits::default().max_rooms)]
    pub max_rooms: usize,

    /// Most peers in one room (at most 256).
    #[arg(long, env = "DRAFFTINK_MAX_PEERS_PER_ROOM", default_value_t = ServerLimits::default().max_peers_per_room)]
    pub max_peers_per_room: usize,

    /// Most open WebSocket connections.
    #[arg(long, env = "DRAFFTINK_MAX_CONNECTIONS", default_value_t = ServerLimits::default().max_connections)]
    pub max_connections: usize,

    /// Largest WebSocket message in bytes.
    #[arg(long, env = "DRAFFTINK_MAX_FRAME_BYTES", default_value_t = ServerLimits::default().max_frame_bytes)]
    pub max_frame_bytes: usize,

    /// Largest document update in bytes.
    #[arg(long, env = "DRAFFTINK_MAX_UPDATE_BYTES", default_value_t = ServerLimits::default().max_update_bytes)]
    pub max_update_bytes: usize,

    /// Document updates per second per connection.
    #[arg(long, env = "DRAFFTINK_MAX_UPDATES_PER_SEC", default_value_t = ServerLimits::default().max_updates_per_sec)]
    pub max_updates_per_sec: u16,

    /// Awareness messages per second per connection.
    #[arg(long, env = "DRAFFTINK_MAX_AWARENESS_PER_SEC", default_value_t = ServerLimits::default().max_awareness_per_sec)]
    pub max_awareness_per_sec: u16,

    /// Stored bytes one room may grow to.
    #[arg(long, env = "DRAFFTINK_MAX_ROOM_BYTES", default_value_t = ServerLimits::default().max_room_bytes)]
    pub max_room_bytes: u64,

    /// Seconds a connection may stay open without sending `Join`.
    #[arg(long, env = "DRAFFTINK_JOIN_TIMEOUT_SECS", default_value_t = 10)]
    pub join_timeout_secs: u64,

    /// Milliseconds between store writes of new updates; 0 writes before every `Ack`.
    #[arg(long, env = "DRAFFTINK_FLUSH_INTERVAL_MS", default_value_t = 2000)]
    pub flush_interval_ms: u64,

    /// Seconds an empty room stays loaded before it is saved and unloaded.
    #[arg(long, env = "DRAFFTINK_ROOM_IDLE_SECS", default_value_t = 600)]
    pub room_idle_secs: u64,

    /// Log entries that trigger a compaction.
    #[arg(long, env = "DRAFFTINK_COMPACT_ENTRIES", default_value_t = Timing::default().compact_entries)]
    pub compact_entries: u64,

    /// Log bytes that trigger a compaction.
    #[arg(long, env = "DRAFFTINK_COMPACT_BYTES", default_value_t = Timing::default().compact_bytes)]
    pub compact_bytes: u64,

    /// Delete rooms that nobody wrote to for this many days. Off by default.
    #[arg(long, env = "DRAFFTINK_ROOM_EXPIRY_DAYS")]
    pub room_expiry_days: Option<u64>,
}

impl Cli {
    /// Builds the [`Config`], reading the token file if one is given.
    pub fn into_config(self) -> Result<Config, ConfigError> {
        let mut auth_tokens: Vec<String> = self.auth_tokens.into_iter().filter(|t| !t.is_empty()).collect();
        if let Some(path) = &self.auth_token_file {
            let text = std::fs::read_to_string(path)
                .map_err(|e| bad(format!("cannot read the auth token file {}: {e}", path.display())))?;
            let before = auth_tokens.len();
            auth_tokens.extend(
                text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned),
            );
            if auth_tokens.len() == before {
                return Err(bad(format!("the auth token file {} holds no tokens", path.display())));
            }
        }
        let storage = match self.storage {
            StorageKind::Memory => StorageConfig::Memory,
            StorageKind::Filesystem => StorageConfig::Filesystem { dir: self.data_dir.clone() },
            StorageKind::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    StorageConfig::Sqlite {
                        path: self.sqlite_path.clone().unwrap_or_else(|| self.data_dir.join("drafftink.sqlite3")),
                    }
                }
                #[cfg(not(feature = "sqlite"))]
                {
                    return Err(bad("this build has no SQLite support; rebuild with `--features sqlite`"));
                }
            }
            StorageKind::S3 => {
                #[cfg(feature = "s3")]
                {
                    let bucket =
                        self.s3_bucket.clone().ok_or_else(|| bad("--s3-bucket is required for --storage s3"))?;
                    StorageConfig::S3(crate::store::S3Config {
                        bucket,
                        endpoint: self.s3_endpoint.clone(),
                        region: self.s3_region.clone(),
                        access_key_id: self.s3_access_key_id.clone(),
                        secret_access_key: self.s3_secret_access_key.clone(),
                        session_token: self.s3_session_token.clone(),
                        prefix: self.s3_prefix.clone(),
                        allow_http: self.s3_allow_http,
                        virtual_hosted_style: self.s3_virtual_hosted_style,
                        disable_bulk_delete: self.s3_disable_bulk_delete,
                    })
                }
                #[cfg(not(feature = "s3"))]
                {
                    return Err(bad("this build has no S3 support; rebuild with `--features s3`"));
                }
            }
        };
        let limits = ServerLimits {
            max_frame_bytes: self.max_frame_bytes,
            max_update_bytes: self.max_update_bytes,
            max_updates_per_sec: self.max_updates_per_sec,
            max_awareness_per_sec: self.max_awareness_per_sec,
            max_rooms: self.max_rooms,
            max_peers_per_room: self.max_peers_per_room,
            max_connections: self.max_connections,
            max_room_bytes: self.max_room_bytes,
            ..ServerLimits::default()
        };
        let timing = Timing {
            join_timeout: Duration::from_secs(self.join_timeout_secs),
            flush_interval: Duration::from_millis(self.flush_interval_ms),
            room_idle: Duration::from_secs(self.room_idle_secs),
            compact_entries: self.compact_entries,
            compact_bytes: self.compact_bytes,
            expire_after: self.room_expiry_days.map(|d| Duration::from_secs(d.saturating_mul(86_400))),
            ..Timing::default()
        };
        let config = Config {
            bind: self.bind,
            web_dir: self.web_dir,
            storage,
            allowed_origins: self.allowed_origins.into_iter().filter(|o| !o.is_empty()).collect(),
            auth_tokens,
            debug_json: self.debug_json,
            source_url: self.source_url,
            limits,
            timing,
        };
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, ConfigError> {
        let mut full = vec!["drafftink-server"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).map_err(|e| bad(e.to_string()))?.into_config()
    }

    #[test]
    fn the_source_url_defaults_to_the_repository_and_can_be_set() {
        assert_eq!(parse(&[]).unwrap().source_url, env!("CARGO_PKG_REPOSITORY"));
        let set = parse(&["--source-url", "https://git.example.org/me/fork"]).unwrap();
        assert_eq!(set.source_url, "https://git.example.org/me/fork");
        let err = parse(&["--source-url", "file:///etc/passwd"]).unwrap_err();
        assert!(err.0.contains("source-url"), "{}", err.0);
    }

    #[test]
    fn defaults_are_loopback_filesystem_auth_off() {
        let c = parse(&[]).expect("defaults");
        assert_eq!(c.bind, SocketAddr::from(([127, 0, 0, 1], 3030)));
        assert!(matches!(c.storage, StorageConfig::Filesystem { .. }));
        assert!(c.auth_tokens.is_empty() && c.allowed_origins.is_empty() && !c.debug_json);
        assert_eq!(c.web_dir, PathBuf::from("web-dist"));
    }

    #[test]
    fn explicit_values_are_used() {
        let c = parse(&[
            "--bind",
            "0.0.0.0:8080",
            "--storage",
            "memory",
            "--allowed-origin",
            "https://a.example",
            "--allowed-origin",
            "https://b.example,https://c.example",
            "--auth-token",
            "s3cret",
            "--max-peers-per-room",
            "5",
            "--debug-json",
        ])
        .expect("parses");
        assert_eq!(c.bind.port(), 8080);
        assert!(matches!(c.storage, StorageConfig::Memory));
        assert_eq!(c.allowed_origins.len(), 3);
        assert_eq!(c.auth_tokens, ["s3cret"]);
        assert_eq!(c.limits.max_peers_per_room, 5);
        assert!(c.debug_json);
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(parse(&["--bind", "nowhere"]).is_err());
        assert!(parse(&["--max-peers-per-room", "0"]).is_err());
        assert!(parse(&["--max-peers-per-room", "1000"]).is_err());
        assert!(parse(&["--max-update-bytes", "99999999"]).is_err());
        assert!(parse(&["--max-frame-bytes", "100"]).is_err());
        assert!(parse(&["--max-updates-per-sec", "0"]).is_err());
        assert!(parse(&["--storage", "tape"]).is_err());
    }

    #[test]
    fn token_file_is_read_and_must_not_be_empty() {
        let dir = std::env::temp_dir().join(format!("drafftink-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let good = dir.join("good");
        std::fs::write(&good, "# comment\none\n\n two \n").expect("write");
        let c = parse(&["--auth-token-file", good.to_str().expect("utf8")]).expect("parses");
        assert_eq!(c.auth_tokens, ["one", "two"]);
        let empty = dir.join("empty");
        std::fs::write(&empty, "\n# nothing\n").expect("write");
        assert!(parse(&["--auth-token-file", empty.to_str().expect("utf8")]).is_err());
        assert!(parse(&["--auth-token-file", dir.join("missing").to_str().expect("utf8")]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(feature = "s3"))]
    #[test]
    fn s3_without_the_feature_says_how_to_fix_it() {
        let err = parse(&["--storage", "s3"]).expect_err("no s3");
        assert!(err.0.contains("--features s3"));
    }
}
