//! The relays under test: how to start each one, and what differs between them as data.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use super::{child, in_process, wrangler};

/// A relay target of the suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `drafftink_server::start` in this process.
    InProcess,
    /// The `drafftink-headless serve` child process.
    Child,
    /// The Worker under `wrangler dev` (gated by `DRAFFTINK_PARITY_WRANGLER`).
    Wrangler,
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
    /// Where it keeps its state on disk. Searched by the canary once the relay has stopped.
    stored: std::path::PathBuf,
    host: Box<dyn Host>,
}

/// The thing that keeps a relay running and stops it.
pub trait Host {
    /// Stops the relay and waits until it is gone, so that the files it wrote are complete. Safe to call twice.
    fn stop(&mut self);
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
        Relay { name, addr, url: format!("ws://{addr}/ws"), traits, life, stored: stored.to_path_buf(), host }
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
        }
    }
}
