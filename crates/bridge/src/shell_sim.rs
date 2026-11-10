//! A stand-in for the desktop shell, for tests (native only): a thread that owns a `Session`, polls the
//! [`BridgeHost`] when the waker fires, and hands the session's last state back when it stops.
//!
//! A real shell polls at the top of its frame and sleeps otherwise. This one waits on a condition variable
//! with a short timeout so that change notifications for edits made by other means also get out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use drafftink_engine::Session;
use serde_json::Value;

use crate::server::{BridgeConfig, BridgeError, BridgeServer};

/// A running server with a simulated shell behind it.
pub struct ShellSim {
    server: BridgeServer,
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
    thread: Option<JoinHandle<Value>>,
}

impl ShellSim {
    /// Starts the server; `make_session` runs on the shell thread.
    pub fn start(
        cfg: BridgeConfig,
        make_session: impl FnOnce() -> Session + Send + 'static,
    ) -> Result<Self, BridgeError> {
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let waker_wake = wake.clone();
        let (server, mut host) = BridgeServer::start(cfg, Arc::new(move || waker_wake.1.notify_all()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_stop, thread_wake) = (stop.clone(), wake.clone());
        let thread = std::thread::Builder::new()
            .name("shell-sim".to_owned())
            .spawn(move || {
                let mut session = make_session();
                while !thread_stop.load(Ordering::SeqCst) {
                    host.poll(&mut session);
                    let guard = thread_wake.0.lock().unwrap_or_else(PoisonError::into_inner);
                    let _ = thread_wake.1.wait_timeout(guard, Duration::from_millis(20));
                }
                session.state_json()
            })
            .map_err(BridgeError::Thread)?;
        Ok(Self { server, stop, wake, thread: Some(thread) })
    }

    /// The server.
    pub fn server(&self) -> &BridgeServer {
        &self.server
    }

    /// Stops the shell and returns the session's final `engine.state`.
    pub fn stop(mut self) -> Value {
        self.halt()
    }

    fn halt(&mut self) -> Value {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.1.notify_all();
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or(Value::Null)
    }
}

impl Drop for ShellSim {
    fn drop(&mut self) {
        self.halt();
    }
}
