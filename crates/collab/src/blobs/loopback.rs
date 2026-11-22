//! An in-memory [`BlobTransport`] and the server end that scripts it, modelled on [`loopback`](crate::loopback).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::{BlobReply, BlobRequest, BlobResponse, BlobTransport, BlobTransportError};
use crate::access::{AccessGrant, AccessOrigin};
use crate::transport::{WakeFn, lock};

#[derive(Default)]
struct Link {
    to_server: VecDeque<BlobRequest>,
    to_client: VecDeque<BlobResponse>,
    sent: usize,
    wake: Option<WakeFn>,
    access: Option<AccessGrant>,
    /// For each request: the client id of the service token the transport would have sent, if any.
    access_at_send: Vec<Option<String>>,
}

/// An in-memory pair for tests: the client end is a [`BlobTransport`], the server end takes requests and answers.
pub fn loopback_blobs() -> (LoopbackBlobs, LoopbackBlobServer) {
    let link = Arc::new(Mutex::new(Link::default()));
    (LoopbackBlobs { link: Arc::clone(&link) }, LoopbackBlobServer { link })
}

/// The client end of [`loopback_blobs`].
pub struct LoopbackBlobs {
    link: Arc<Mutex<Link>>,
}

impl LoopbackBlobs {
    /// Sets the wake-up hook, called when the server end queues an answer.
    pub fn with_wake(self, wake: WakeFn) -> Self {
        lock(&self.link).wake = Some(wake);
        self
    }
}

impl BlobTransport for LoopbackBlobs {
    fn set_access(&mut self, access: Option<AccessGrant>) {
        lock(&self.link).access = access;
    }

    fn send(&mut self, request: BlobRequest) -> Result<(), BlobTransportError> {
        let mut l = lock(&self.link);
        l.sent += 1;
        let sent = l.access.as_ref().and_then(|g| {
            let origin = AccessOrigin::of_http_url(&request.url)?;
            g.headers_for(&origin).map(|h| h.pairs()[0].1.to_owned())
        });
        l.access_at_send.push(sent);
        l.to_server.push_back(request);
        Ok(())
    }

    fn poll(&mut self) -> Option<BlobResponse> {
        lock(&self.link).to_client.pop_front()
    }
}

/// The server end of [`loopback_blobs`].
pub struct LoopbackBlobServer {
    link: Arc<Mutex<Link>>,
}

impl LoopbackBlobServer {
    /// How many requests the client has sent so far.
    pub fn sent(&self) -> usize {
        lock(&self.link).sent
    }

    /// For each request, the client id of the service token the transport sent with it, or `None` when it sent none.
    /// Only the client id is kept: the secret never leaves the grant.
    pub fn access_client_ids(&self) -> Vec<Option<String>> {
        lock(&self.link).access_at_send.clone()
    }

    /// The next request, oldest first.
    pub fn take_request(&self) -> Option<BlobRequest> {
        lock(&self.link).to_server.pop_front()
    }

    /// Every request waiting, oldest first.
    pub fn take_requests(&self) -> Vec<BlobRequest> {
        lock(&self.link).to_server.drain(..).collect()
    }

    /// Answers request `id`.
    pub fn respond(&self, id: u64, result: Result<BlobReply, String>) {
        let wake = {
            let mut l = lock(&self.link);
            l.to_client.push_back(BlobResponse { id, result });
            l.wake.clone()
        };
        if let Some(w) = wake {
            w();
        }
    }

    /// Answers request `id` with a status and a body.
    pub fn reply(&self, id: u64, status: u16, body: Vec<u8>) {
        let content_length = u64::try_from(body.len()).ok();
        self.respond(id, Ok(BlobReply { status, body, content_length, ..BlobReply::default() }));
    }

    /// Answers request `id` with a failure of the network.
    pub fn fail(&self, id: u64, why: &str) {
        self.respond(id, Err(why.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::{BlobAuth, BlobOp};
    use drafftink_protocol::v2::MembershipToken;

    fn request(id: u64) -> BlobRequest {
        BlobRequest {
            id,
            url: "http://127.0.0.1/r/x/f".to_owned(),
            op: BlobOp::List,
            auth: BlobAuth::member(&MembershipToken::from_bytes([1; 32])),
        }
    }

    #[test]
    fn requests_go_to_the_server_and_answers_come_back_in_order() {
        let (mut client, server) = loopback_blobs();
        client.send(request(1)).unwrap();
        client.send(request(2)).unwrap();
        assert_eq!(server.sent(), 2);
        assert_eq!(server.take_request().map(|r| r.id), Some(1));
        server.reply(1, 200, b"[]".to_vec());
        server.fail(2, "reset");
        let first = client.poll().unwrap();
        assert_eq!((first.id, first.result.unwrap().status), (1, 200));
        assert_eq!(client.poll().unwrap().result, Err("reset".to_owned()));
        assert!(client.poll().is_none());
    }

    #[test]
    fn the_wake_hook_runs_for_each_answer() {
        let woken = Arc::new(Mutex::new(0u32));
        let counter = Arc::clone(&woken);
        let (client, server) = loopback_blobs();
        let _client = client.with_wake(Arc::new(move || *lock(&counter) += 1));
        server.reply(1, 200, Vec::new());
        server.reply(2, 404, Vec::new());
        assert_eq!(*lock(&woken), 2);
    }
}
