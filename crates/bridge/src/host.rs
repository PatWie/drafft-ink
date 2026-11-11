//! The shell side of the bridge. Transport threads never touch the [`Session`]. They put a [`Job`] on a
//! bounded queue and call the waker. The shell owns a [`BridgeHost`] and calls [`BridgeHost::poll`] at the
//! top of a frame; that is the only place bridge requests run, on the shell's thread, between frames.
//!
//! ```ignore
//! let (server, mut host) = BridgeServer::start(config, Arc::new(move || ctx.request_repaint()))?;
//! // in the frame:
//! let polled = host.poll(&mut session);
//! if polled.more { /* the host already woke the shell again */ }
//! ```
//!
//! Idle stays idle: nothing here has a timer. A request wakes the shell once; a flood is run 64 per poll
//! and the host wakes the shell again until the queue is empty.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

use drafftink_engine::{Finished, PendingRender, Session};
use serde_json::{Value, json};

use crate::core::dispatch;
use crate::rpc::{BUSY, INTERNAL_ERROR, REFUSED, RpcError};

/// Requests run per poll. The rest stay queued for the next frame, so a flood cannot starve drawing.
pub const PER_POLL: usize = 64;
/// Requests that may wait. More are answered `busy` at once.
pub const QUEUE: usize = 256;

/// Wakes the shell so that it polls (for example `ctx.request_repaint()`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;
/// Delivers the answer to the transport.
pub type Reply = Box<dyn FnOnce(Result<Value, RpcError>) + Send>;

/// One request waiting for the shell.
pub struct Job {
    /// The method.
    pub method: String,
    /// The parameters.
    pub params: Value,
    /// Where the answer goes.
    pub reply: Reply,
}

/// Where change notifications go. The native server implements it with a broadcast channel.
pub trait EventSink: Send + Sync {
    /// True when at least one peer wants notifications. Without one the host does no work for them.
    fn has_subscribers(&self) -> bool;
    /// Sends `{method, params}` to every subscriber.
    fn publish(&self, method: &str, params: Value);
}

/// The transport's end of the queue.
#[derive(Clone)]
pub struct JobSender {
    tx: SyncSender<Job>,
    waker: Waker,
}

impl JobSender {
    /// Queues a job and wakes the shell. A full queue or a closed shell is an error the transport sends back.
    pub fn submit(&self, job: Job) -> Result<(), (RpcError, Reply)> {
        match self.tx.try_send(job) {
            Ok(()) => {
                (self.waker)();
                Ok(())
            }
            Err(TrySendError::Full(j)) => Err((
                RpcError::new(
                    BUSY,
                    "the app has too many requests waiting. Wait for the earlier answers, then try again",
                ),
                j.reply,
            )),
            Err(TrySendError::Disconnected(j)) => {
                Err((RpcError::new(INTERNAL_ERROR, "the app is closing, so the request was not run"), j.reply))
            }
        }
    }
}

/// What one poll did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Polled {
    /// Requests run.
    pub ran: usize,
    /// More requests wait. The host has already woken the shell for them.
    pub more: bool,
}

/// The code of the refusal that parks a request: the command needs a picture that the port is still drawing.
const RENDER_PENDING: &str = "render_pending";

/// True when the answer says "run me again when the port is done".
///
/// A single command comes back as a refusal. A batch whose `pending_render` is true was taken back by the engine,
/// because a later step changes what the picture shows, so running the whole batch again is safe. (A batch whose
/// result lists `pending` steps is complete: [`WaitingBatch`] collects the pictures.)
fn is_render_pending(out: &Result<Value, RpcError>) -> bool {
    match out {
        Err(e) => {
            e.code == REFUSED
                && e.data.as_ref().and_then(|d| d.get("code")).and_then(Value::as_str) == Some(RENDER_PENDING)
        }
        Ok(result) => result.get("pending_render").and_then(Value::as_bool) == Some(true),
    }
}

/// A batch that is complete and committed, except for the pictures of some steps (`Session::finish_render`). The
/// transport gets its answer when the last picture is in.
struct WaitingBatch {
    reply: Reply,
    /// The batch result, with a placeholder at the index of each step that waits.
    result: Value,
    /// Steps that wait.
    pending: Vec<PendingRender>,
}

impl WaitingBatch {
    /// The batch whose result lists steps that wait for a picture, if `out` is one.
    fn of(out: &Result<Value, RpcError>) -> Option<(Value, Vec<PendingRender>)> {
        let result = out.as_ref().ok()?;
        let pending: Vec<PendingRender> = serde_json::from_value(result.get("pending")?.clone()).ok()?;
        (!pending.is_empty()).then(|| (result.clone(), pending))
    }

    /// Asks the session for each picture that is still missing and puts it into the result. True when none is.
    fn advance(&mut self, session: &mut Session) -> bool {
        let waiting = std::mem::take(&mut self.pending);
        for step in waiting {
            match session.finish_render(step.token) {
                Ok(Finished::Waiting) => self.pending.push(step),
                Ok(Finished::Done(output)) => self.put(step.index, output),
                Err(refusal) => {
                    self.put(step.index, Value::Null);
                    self.fail(&step, &refusal);
                }
            }
        }
        self.pending.is_empty()
    }

    fn put(&mut self, index: usize, output: Value) {
        if let Some(slot) = self.result.get_mut("results").and_then(|r| r.get_mut(index)) {
            *slot = output;
        }
    }

    /// A picture that could not be made: the batch stays applied, and the result says which step failed.
    fn fail(&mut self, step: &PendingRender, refusal: &drafftink_engine::Refusal) {
        let Value::Object(map) = &mut self.result else { return };
        let PendingRender { index, id, .. } = step;
        map.entry("failed").or_insert_with(
            || json!({ "index": index, "id": id, "refusal": serde_json::to_value(refusal).unwrap_or(Value::Null) }),
        );
        map.insert(
            "note".to_owned(),
            json!(format!("the picture for step {index} (`{id}`) was not made: {}", refusal.message)),
        );
    }
}

/// The shell's end of the queue.
pub struct BridgeHost {
    rx: Receiver<Job>,
    /// Requests that were refused with `render_pending`. Each poll runs them again before the new ones, and the
    /// transport gets the answer of the run that was not pending (the picture, or the real failure).
    parked: Vec<Job>,
    /// Batches that are applied and wait for pictures. Each poll asks for them before the new requests run.
    waiting: Vec<WaitingBatch>,
    waker: Waker,
    sink: Option<Arc<dyn EventSink>>,
    announced: (u64, u64),
}

/// Makes the two ends of the queue.
pub fn channel(waker: Waker, sink: Option<Arc<dyn EventSink>>) -> (JobSender, BridgeHost) {
    let (tx, rx) = sync_channel(QUEUE);
    (
        JobSender { tx, waker: waker.clone() },
        BridgeHost { rx, parked: Vec::new(), waiting: Vec::new(), waker, sink, announced: (0, 0) },
    )
}

impl BridgeHost {
    /// Runs up to [`PER_POLL`] waiting requests against `session`, then announces a change to subscribers.
    /// Cheap when nothing waits: one failed `try_recv` and two integer compares.
    pub fn poll(&mut self, session: &mut Session) -> Polled {
        let mut ran = 0;
        let mut more = false;
        for mut batch in std::mem::take(&mut self.waiting) {
            if batch.advance(session) {
                (batch.reply)(Ok(batch.result));
            } else {
                self.waiting.push(batch);
            }
        }
        for job in std::mem::take(&mut self.parked) {
            self.run(session, job);
        }
        while ran < PER_POLL {
            match self.rx.try_recv() {
                Ok(job) => {
                    self.run(session, job);
                    ran += 1;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if ran == PER_POLL {
            // The queue may hold more. A sync_channel cannot be peeked, so ask the shell for one more poll.
            more = true;
            (self.waker)();
        }
        self.announce(session);
        Polled { ran, more }
    }

    /// Runs one request and answers it, or parks it when the answer is "not drawn yet". A request that cannot be
    /// parked (the shelf is full) gets the refusal, whose text says to run it again.
    fn run(&mut self, session: &mut Session, job: Job) {
        let out = dispatch(session, &job.method, &job.params);
        if is_render_pending(&out) && self.parked.len() < QUEUE {
            self.parked.push(job);
        } else if let Some((result, pending)) = WaitingBatch::of(&out)
            && self.waiting.len() < QUEUE
        {
            let mut batch = WaitingBatch { reply: job.reply, result, pending };
            // The picture may be ready already (the port drew it between the step and now).
            if batch.advance(session) {
                (batch.reply)(Ok(batch.result));
            } else {
                self.waiting.push(batch);
            }
        } else {
            (job.reply)(out);
        }
    }

    fn announce(&mut self, session: &Session) {
        let Some(sink) = &self.sink else { return };
        let now = (session.ring().next_seq(), session.tab().doc.revision());
        if now == self.announced {
            return;
        }
        self.announced = now;
        if sink.has_subscribers() {
            sink.publish("changed", json!({ "seq": now.0, "revision": now.1 }));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Mutex;

    use drafftink_engine::PortError;
    use drafftink_engine::ports::{RenderRequest, Renderer, SceneView};
    use drafftink_engine::testing::{add_rect, session};
    use serde_json::json;

    use super::*;

    /// A renderer that is not ready for the first `not_ready` calls, as the browser's is while the GPU reads back.
    struct SlowRenderer {
        not_ready: Rc<Cell<u32>>,
    }

    impl Renderer for SlowRenderer {
        fn render_png(&self, _: &SceneView, _: &RenderRequest) -> Result<Vec<u8>, PortError> {
            match self.not_ready.get() {
                0 => Ok(b"png".to_vec()),
                n => {
                    self.not_ready.set(n - 1);
                    Err(PortError::NotReady)
                }
            }
        }
    }

    type Answers = Arc<Mutex<Vec<Result<Value, RpcError>>>>;

    fn submit(tx: &JobSender, method: &str, params: Value) -> Answers {
        let answers: Answers = Arc::default();
        let sink = answers.clone();
        let job = Job {
            method: method.to_owned(),
            params,
            reply: Box::new(move |r| sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(r)),
        };
        assert!(tx.submit(job).is_ok());
        answers
    }

    fn count(a: &Answers) -> usize {
        a.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    #[test]
    fn a_request_the_renderer_is_not_ready_for_waits_and_is_answered_when_it_is() {
        let (mut s, _fakes) = session();
        add_rect(&mut s, 0.0, 0.0, 50.0, 40.0).unwrap();
        let not_ready = Rc::new(Cell::new(2));
        s.ports.renderer = Some(Box::new(SlowRenderer { not_ready: not_ready.clone() }));
        let (tx, mut host) = channel(Arc::new(|| {}), None);

        let export = submit(&tx, "execute", json!({ "id": "doc.export", "args": { "format": "png" } }));
        let other = submit(&tx, "state", json!({}));
        host.poll(&mut s);
        assert_eq!(count(&other), 1, "other requests are not held up");
        assert_eq!(count(&export), 0, "the export waits: the picture is not there");
        host.poll(&mut s);
        assert_eq!(count(&export), 0, "still not ready after the second try");
        host.poll(&mut s);
        let answers = export.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = answers.first().and_then(|r| r.as_ref().ok()).expect("the third try gets the picture");
        assert!(value.get("bytes").is_some(), "{value}");
    }

    #[test]
    fn a_batch_that_exports_a_png_is_applied_at_once_and_answered_when_the_picture_is_in() {
        let (mut s, _fakes) = session();
        let id = add_rect(&mut s, 0.0, 0.0, 50.0, 40.0).unwrap().to_string();
        s.ports.renderer = Some(Box::new(SlowRenderer { not_ready: Rc::new(Cell::new(3)) }));
        let (tx, mut host) = channel(Arc::new(|| {}), None);

        let steps = json!([
            { "id": "element.move", "args": { "ids": [id], "dx": 10, "dy": 0 } },
            { "id": "doc.export", "args": { "format": "png" } },
        ]);
        let batch = submit(&tx, "batch", json!({ "steps": steps }));
        host.poll(&mut s);
        assert_eq!(count(&batch), 0, "the answer waits for the picture");
        assert_eq!(s.element(&id.parse().unwrap()).unwrap().transform.x, 10.0, "the batch itself is already applied");
        assert_eq!(s.tab().doc.undo_depth(), 1);
        host.poll(&mut s);
        assert_eq!(count(&batch), 0, "still not ready after the second try");
        host.poll(&mut s);
        let answers = batch.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = answers.first().and_then(|r| r.as_ref().ok()).expect("the third poll completes the batch");
        assert!(value.get("failed").is_none(), "{value}");
        assert_eq!(value["completed"], 2);
        assert_eq!(value["results"].as_array().map(Vec::len), Some(2));
        assert!(value["results"][1].get("bytes").is_some(), "the placeholder was replaced by the output: {value}");
        assert_eq!(s.tab().doc.undo_depth(), 1, "the batch was not run twice");
    }

    #[test]
    fn a_batch_whose_picture_goes_stale_is_answered_with_the_step_that_failed_and_keeps_its_other_steps() {
        let (mut s, _fakes) = session();
        let id = add_rect(&mut s, 0.0, 0.0, 50.0, 40.0).unwrap().to_string();
        s.ports.renderer = Some(Box::new(SlowRenderer { not_ready: Rc::new(Cell::new(3)) }));
        let (tx, mut host) = channel(Arc::new(|| {}), None);
        let steps = json!([
            { "id": "element.move", "args": { "ids": [id], "dx": 10, "dy": 0 } },
            { "id": "doc.export", "args": { "format": "png" } },
        ]);
        let batch = submit(&tx, "batch", json!({ "steps": steps }));
        host.poll(&mut s);
        assert_eq!(count(&batch), 0);
        // The user draws something before the picture is read back.
        add_rect(&mut s, 200.0, 0.0, 10.0, 10.0).unwrap();
        host.poll(&mut s);
        let answers = batch.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = answers.first().and_then(|r| r.as_ref().ok()).expect("a stale picture ends the wait");
        assert_eq!(value["failed"]["index"], 1);
        assert_eq!(value["failed"]["refusal"]["code"], "render_stale");
        assert!(value["results"][1].is_null());
        assert_eq!(s.element(&id.parse().unwrap()).unwrap().transform.x, 10.0, "the batch stays applied");
    }

    #[test]
    fn a_batch_whose_later_step_changes_the_picture_is_taken_back_and_run_again() {
        let (mut s, _fakes) = session();
        let id = add_rect(&mut s, 0.0, 0.0, 50.0, 40.0).unwrap().to_string();
        s.ports.renderer = Some(Box::new(SlowRenderer { not_ready: Rc::new(Cell::new(1)) }));
        let (tx, mut host) = channel(Arc::new(|| {}), None);
        let steps = json!([
            { "id": "doc.export", "args": { "format": "png" } },
            { "id": "element.move", "args": { "ids": [id], "dx": 10, "dy": 0 } },
        ]);
        let batch = submit(&tx, "batch", json!({ "steps": steps }));
        host.poll(&mut s);
        assert_eq!(count(&batch), 0, "parked: the move would change the picture");
        assert_eq!(s.element(&id.parse().unwrap()).unwrap().transform.x, 0.0, "nothing was applied yet");
        host.poll(&mut s);
        let answers = batch.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = answers.first().and_then(|r| r.as_ref().ok()).expect("the rerun completes");
        assert!(value.get("failed").is_none() && value.get("pending").is_none(), "{value}");
        assert!(value["results"][0].get("bytes").is_some());
    }

    #[test]
    fn a_batch_that_fails_for_another_reason_is_answered_at_once() {
        let (mut s, _fakes) = session();
        let (tx, mut host) = channel(Arc::new(|| {}), None);
        let batch = submit(&tx, "batch", json!({ "steps": [{ "id": "doc.export", "args": { "format": "png" } }] }));
        host.poll(&mut s);
        assert_eq!(count(&batch), 1, "an empty document is nothing_to_do, not a wait");
        assert!(host.parked.is_empty());
    }

    #[test]
    fn a_real_failure_is_not_parked() {
        let (mut s, _fakes) = session();
        let (tx, mut host) = channel(Arc::new(|| {}), None);
        // The empty document refuses with nothing_to_do, not render_pending.
        let export = submit(&tx, "execute", json!({ "id": "doc.export", "args": { "format": "png" } }));
        host.poll(&mut s);
        assert_eq!(count(&export), 1);
        assert!(host.parked.is_empty());
    }
}
