//! The native picture transport: a few worker threads run blocking `ureq` requests.
//!
//! `send` hands a request to a pool of at most [`WORKERS`] threads (started on first use), so the three downloads and
//! two uploads that the client allows run side by side. A worker puts the answer in a queue and calls the wake hook;
//! the owner drains the queue with [`BlobTransport::poll`]. The owner's thread never blocks.
//!
//! `https://` uses rustls with the ring provider and the bundled Mozilla roots (the same rustls as the WebSocket
//! transport, no aws-lc). Plain `http://` is only for a server on this machine (what `ws://` allows without opt-in),
//! unless [`NativeBlobOptions::allow_insecure`] is set. Redirects are never followed (the `Authorization` header
//! would go along), proxies from the environment are not used (the WebSocket does not use them either), and a
//! response body over [`MAX_BLOB_BODY_BYTES`] is a failure.

use std::collections::VecDeque;
use std::io::Read as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ureq::http::{HeaderMap, Uri, header};

use super::{
    BlobOp, BlobReply, BlobRequest, BlobResponse, BlobTransport, BlobTransportError, MAX_BLOB_BODY_BYTES,
    retry_after_ms,
};
use crate::transport::{WakeFn, lock};
use crate::url::is_loopback_host;

/// Most requests in flight at once (worker threads). The client asks for at most 3 downloads and 2 uploads.
const WORKERS: usize = 5;
/// A request that has not finished after this long fails. The client gives up a little later (120 s).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(100);
/// Time to connect, including the TLS handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Tuning of [`NativeBlobTransport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NativeBlobOptions {
    /// Allow plain `http://` to a host that is not this machine. Off by default, like `ws://` for the WebSocket: the
    /// pictures are sealed, but the membership token in the header would be readable on the path.
    pub allow_insecure: bool,
}

/// What a worker is asked to do.
struct Job {
    request: BlobRequest,
}

/// The state workers share with the transport.
struct Shared {
    jobs: Mutex<Receiver<Job>>,
    answers: Mutex<VecDeque<BlobResponse>>,
    /// Requests sent and not yet answered.
    in_flight: AtomicUsize,
}

/// Picture requests on worker threads. See the module docs.
pub struct NativeBlobTransport {
    wake: WakeFn,
    options: NativeBlobOptions,
    agent: ureq::Agent,
    shared: Arc<Shared>,
    jobs: Sender<Job>,
    threads: usize,
}

impl NativeBlobTransport {
    /// A transport that calls `wake` (from a worker thread) whenever it has queued an answer.
    pub fn new(wake: WakeFn) -> Self {
        Self::with_options(wake, NativeBlobOptions::default())
    }

    /// Like [`new`](Self::new), with explicit options.
    pub fn with_options(wake: WakeFn, options: NativeBlobOptions) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent("drafftink")
            .build();
        let (jobs, receiver) = channel();
        let shared =
            Arc::new(Shared { jobs: Mutex::new(receiver), answers: Mutex::default(), in_flight: AtomicUsize::new(0) });
        NativeBlobTransport { wake, options, agent: config.into(), shared, jobs, threads: 0 }
    }

    /// Checks that the address may be used before a thread is spent on it.
    fn check(&self, url: &str) -> Result<(), BlobTransportError> {
        let uri: Uri =
            url.parse().map_err(|_| BlobTransportError::Refused("the picture address is not valid".into()))?;
        let host = uri.host().unwrap_or_default().trim_matches(['[', ']']).to_ascii_lowercase();
        match uri.scheme_str() {
            Some("https") => Ok(()),
            Some("http") if self.options.allow_insecure || is_loopback_host(&host) => Ok(()),
            Some("http") => Err(BlobTransportError::Refused(format!(
                "plain http to {host} would show the room's token on the network. Use https"
            ))),
            _ => Err(BlobTransportError::Refused("pictures are only fetched over http or https".into())),
        }
    }

    /// Starts one more worker when there are more requests than workers and the pool is not full.
    fn ensure_worker(&mut self) -> Result<(), BlobTransportError> {
        let wanted = self.shared.in_flight.load(Ordering::Acquire).min(WORKERS);
        if self.threads >= wanted {
            return Ok(());
        }
        let worker =
            Worker { agent: self.agent.clone(), shared: Arc::clone(&self.shared), wake: Arc::clone(&self.wake) };
        // Exception to the no-threads rule below L6 (xtask/clippy-below-l6): blocking HTTP needs its own threads, as
        // the WebSocket transport has its network thread. The browser transport has none.
        #[allow(clippy::disallowed_methods)]
        let spawned = std::thread::Builder::new().name("drafftink-blobs".to_owned()).spawn(move || worker.run());
        match spawned {
            Ok(_) => {
                self.threads += 1;
                Ok(())
            }
            // With no thread at all nothing can run. With some, the job waits for a free one.
            Err(e) if self.threads == 0 => {
                Err(BlobTransportError::Failed(format!("cannot start a network thread: {e}")))
            }
            Err(_) => Ok(()),
        }
    }
}

impl BlobTransport for NativeBlobTransport {
    fn send(&mut self, request: BlobRequest) -> Result<(), BlobTransportError> {
        self.check(&request.url)?;
        self.shared.in_flight.fetch_add(1, Ordering::AcqRel);
        let started = self.ensure_worker().and_then(|()| {
            self.jobs
                .send(Job { request })
                .map_err(|_| BlobTransportError::Failed("the network threads stopped".into()))
        });
        if started.is_err() {
            self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
        }
        started
    }

    fn poll(&mut self) -> Option<BlobResponse> {
        lock(&self.shared.answers).pop_front()
    }
}

/// One worker thread: takes jobs until the transport is dropped and the channel closes.
struct Worker {
    agent: ureq::Agent,
    shared: Arc<Shared>,
    wake: WakeFn,
}

impl Worker {
    fn run(self) {
        loop {
            // The lock is held while waiting, so exactly one idle worker waits on the channel at a time.
            let Ok(job) = lock(&self.shared.jobs).recv() else { return };
            let result = execute(&self.agent, &job.request);
            lock(&self.shared.answers).push_back(BlobResponse { id: job.request.id, result });
            self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
            (self.wake)();
        }
    }
}

/// Runs one request. A status of 400 or more is a reply, not an error; the text of an `Err` never holds the header.
fn execute(agent: &ureq::Agent, request: &BlobRequest) -> Result<BlobReply, String> {
    let auth = request.auth.header_value();
    let response = match &request.op {
        BlobOp::Put(body) => agent.put(&request.url).header(header::AUTHORIZATION, auth).send(body.as_slice()),
        BlobOp::Get | BlobOp::List => agent.get(&request.url).header(header::AUTHORIZATION, auth).call(),
        BlobOp::Head => agent.head(&request.url).header(header::AUTHORIZATION, auth).call(),
    }
    .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let (content_length, retry_after_ms) = headers_of(response.headers());
    let mut body = Vec::new();
    if !matches!(request.op, BlobOp::Head) {
        let limit = u64::try_from(MAX_BLOB_BODY_BYTES).unwrap_or(u64::MAX).saturating_add(1);
        response.into_body().into_reader().take(limit).read_to_end(&mut body).map_err(|e| e.to_string())?;
        if body.len() > MAX_BLOB_BODY_BYTES {
            return Err("the answer is larger than any picture".to_owned());
        }
    }
    Ok(BlobReply { status, body, content_length, retry_after_ms })
}

fn headers_of(headers: &HeaderMap) -> (Option<u64>, Option<u64>) {
    let text = |name| headers.get(name).and_then(|v| v.to_str().ok());
    (
        text(header::CONTENT_LENGTH).and_then(|v| v.trim().parse().ok()),
        text(header::RETRY_AFTER).and_then(retry_after_ms),
    )
}
