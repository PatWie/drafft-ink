//! The relays under test: how to start each one, and what differs between them as data.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use drafftink_collab::{AccessCredentials, AccessGrant, AccessOrigin, ServerUrl, UrlPolicy};

use super::common::http::{self, Reply};
use super::common::room_client::access_headers;
use super::{child, in_process, stand_in, wrangler};

/// A relay target of the suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `drafftink_server::start` in this process.
    InProcess,
    /// The `drafftink-headless serve` child process.
    Child,
    /// The demo Worker under `wrangler dev` (gated by `DRAFFTINK_PARITY_WRANGLER`).
    Wrangler,
    /// The private Worker under `wrangler dev --env private`: R2 for the pictures, rooms that never expire, and
    /// Cloudflare Access required, with a stand-in for Access in front (gated like `Wrangler`).
    Private,
}

/// How long rooms live on a relay. Every target gets the same numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// The settings of the demo: 48 hours, a minute of grace. Nothing expires during a test.
    Normal,
    /// A room that nobody visits is gone after seconds (the `expire` scenario).
    Short,
}

impl Lifetime {
    /// Time from a room's last visit to its deadline.
    pub fn ttl(self) -> Duration {
        match self {
            Lifetime::Normal => Duration::from_secs(48 * 3600),
            Lifetime::Short => Duration::from_secs(3),
        }
    }

    /// How long a room past its deadline lives on after its last person left.
    pub fn grace(self) -> Duration {
        match self {
            Lifetime::Normal => Duration::from_secs(60),
            Lifetime::Short => Duration::from_secs(1),
        }
    }
}

/// What differs between targets, as values. A scenario reads these and never asks which target it runs on.
#[derive(Debug, Clone, Copy)]
pub struct Traits {
    /// Extra time, after the deadline and the grace, that the target may take to delete a room (a Durable Object alarm
    /// is a timer of another process).
    pub deletion_slack: Duration,
    /// Whether rooms have a deadline: `Joined.expires_at_ms` is not 0. False where rooms never expire.
    pub room_deadline: bool,
}

/// A running relay.
pub struct Relay {
    /// The target's name, for messages.
    pub name: &'static str,
    /// Where it listens.
    pub addr: SocketAddr,
    /// What a client is given: `ws://<addr>/ws`. The client derives `/r/<id>/ws` and the picture routes from it.
    pub url: String,
    /// The values in which this target differs from the others.
    pub traits: Traits,
    /// The lifetime of rooms it was started with.
    pub life: Lifetime,
    /// The service token of a relay behind Cloudflare Access (the stand-in), which every client and hand-made request
    /// must present.
    pub token: Option<AccessCredentials>,
    /// What the stand-in for Access did with each request, when there is one.
    pub gate: Option<stand_in::Log>,
    /// Where it keeps its state on disk. Searched by the canary once the relay has stopped.
    stored: std::path::PathBuf,
    host: Box<dyn Host>,
}

/// The thing that keeps a relay running and stops it.
pub trait Host {
    /// Stops the relay and waits until it is gone, so that the files it wrote are complete. Safe to call twice.
    fn stop(&mut self);

    /// Stops the relay process and starts it again on the same address with the same state on disk. Only a target
    /// that keeps its state can; the others say so.
    fn restart(&mut self) -> Result<(), String> {
        Err("this target cannot restart with its state".to_owned())
    }
}

impl Relay {
    pub fn new(
        name: &'static str,
        addr: SocketAddr,
        traits: Traits,
        life: Lifetime,
        stored: &Path,
        host: Box<dyn Host>,
    ) -> Relay {
        Relay {
            name,
            addr,
            url: format!("ws://{addr}/ws"),
            traits,
            life,
            token: None,
            gate: None,
            stored: stored.to_path_buf(),
            host,
        }
    }

    /// Puts the relay behind a gate that wants `token`.
    pub fn behind(mut self, token: AccessCredentials, gate: stand_in::Log) -> Relay {
        self.token = Some(token);
        self.gate = Some(gate);
        self
    }

    /// The token bound to this relay's origin, as the clients are given it.
    pub fn grant(&self) -> Option<AccessGrant> {
        let url = ServerUrl::parse(&self.url, UrlPolicy { allow_insecure: true }).ok()?;
        self.token.clone().map(|token| AccessGrant::new(AccessOrigin::of(&url), token))
    }

    /// The headers of the token, for a request that is made by hand.
    pub fn access_headers(&self) -> Vec<(&'static str, String)> {
        access_headers(self.grant().as_ref())
    }

    /// A request by hand, with the service token if the relay has a gate.
    pub fn http(&self, method: &str, path: &str, headers: &[(&str, String)], body: &[u8]) -> Reply {
        let mut all: Vec<(&str, String)> = headers.to_vec();
        all.extend(self.access_headers());
        http::request(self.addr, method, path, &all, body)
    }

    /// Stops the relay process and starts it again with its state.
    pub fn restart(&mut self) {
        self.host.restart().unwrap_or_else(|why| panic!("{} cannot restart: {why}", self.name));
    }

    /// Stops the relay. Its files stay readable until the relay is dropped.
    pub fn stop(&mut self) {
        self.host.stop();
    }

    /// The folder with everything the relay wrote.
    pub fn stored(&self) -> &Path {
        &self.stored
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.host.stop();
    }
}

impl Target {
    /// Starts the relay with the given lifetime of rooms. `None` for a gated target that is switched off: it has printed
    /// its `SKIPPED:` line.
    pub fn start(self, life: Lifetime) -> Option<Relay> {
        match self {
            Target::InProcess => Some(in_process::start(life)),
            Target::Child => Some(child::start(life)),
            Target::Wrangler => wrangler::start(life),
            Target::Private => wrangler::start_private(),
        }
    }
}
