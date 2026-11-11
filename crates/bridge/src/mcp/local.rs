//! The caller of the desktop app's own tools: requests go on the shell's queue and run between frames, like
//! those of the bridge.

use std::time::Duration;

use serde_json::{Value, json};

use crate::host::JobSender;
use crate::rpc::RpcError;
use crate::server::request;
use crate::tools::{BoxFuture, Caller, Mode};

/// Time the shell has to run a request.
const REQUEST_LIMIT: Duration = Duration::from_secs(30);

/// Runs bridge methods on the app's shell through its job queue.
#[derive(Clone)]
pub struct LocalCaller {
    jobs: JobSender,
    endpoint: String,
}

impl LocalCaller {
    /// A caller over the queue. `endpoint` is the address the tools are served on, shown by `status`.
    pub fn new(jobs: JobSender, endpoint: impl Into<String>) -> Self {
        Self { jobs, endpoint: endpoint.into() }
    }
}

impl Caller for LocalCaller {
    fn mode(&self) -> Mode {
        Mode::Desktop
    }

    fn call<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(request(&self.jobs, method, params, REQUEST_LIMIT))
    }

    fn status(&self) -> BoxFuture<'_, Value> {
        Box::pin(async move {
            json!({
                "mode": Mode::Desktop.name(),
                "connected": true,
                "listening": self.endpoint,
                "note": "The tools are served by the desktop app itself. PNG export and screenshots use its renderer.",
            })
        })
    }
}
