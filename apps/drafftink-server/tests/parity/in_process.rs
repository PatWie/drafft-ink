//! Target `in_process`: the server library started inside the test process, with the filesystem store.

use std::time::Duration;

use drafftink_relay::Retention;
use drafftink_server::ServerHandle;
use drafftink_server::store::StorageConfig;
use tokio::runtime::Runtime;

use super::common::test_config;
use super::relay::{Host, Lifetime, Relay, Traits};

/// The server and the runtime it runs on. The scenarios are plain blocking code, so the server needs threads of its own.
struct InProcess {
    runtime: Runtime,
    server: Option<ServerHandle>,
    // The folder lives as long as the relay, so that the canary can read the files after the server stopped.
    _dir: tempfile::TempDir,
}

impl Host for InProcess {
    fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            self.runtime.block_on(server.shutdown());
        }
    }
}

pub fn start(life: Lifetime) -> Relay {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut config = test_config();
    config.storage = StorageConfig::Filesystem { dir: dir.path().to_path_buf() };
    config.relay.retention =
        Retention::After { ttl: life.ttl(), grace: life.grace(), recheck: drafftink_protocol::ROOM_RECHECK };
    let runtime =
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a runtime");
    let server = runtime.block_on(drafftink_server::start(config)).expect("the server starts");
    let addr = server.local_addr();
    let stored = dir.path().to_path_buf();
    let host = InProcess { runtime, server: Some(server), _dir: dir };
    Relay::new("in_process", addr, Traits { deletion_slack: Duration::from_secs(1) }, life, &stored, Box::new(host))
}
