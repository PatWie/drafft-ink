//! A stand-in for the web page, for tests of the relay (native only).
//!
//! It does what `window.drafftink` will do in the browser: connect out to the relay, say `hello` with the
//! pairing or resume token, then answer every request with [`crate::core::handle_text`] on its own
//! `Session`. The session lives on the page's own thread, as the page's would live on the browser's main
//! thread, so it needs no `Send`.

use std::sync::mpsc;
use std::thread::JoinHandle;

use drafftink_engine::Session;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::client::{BridgeClient, ClientError, ConnectOpts};
use crate::core::handle_text;
use crate::relay::HOSTED_ORIGIN;

/// A connected fake page.
pub struct FakePage {
    hello: Value,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<Value>>,
}

impl FakePage {
    /// Connects to the relay at `url` with `token`, sending `Origin: origin` (`None`: no header). The session
    /// is built on the page's thread by `make_session`. Returns once the relay accepted the `hello`.
    pub fn connect(
        url: &str,
        token: &str,
        origin: Option<&str>,
        make_session: impl FnOnce() -> Session + Send + 'static,
    ) -> Result<Self, ClientError> {
        let (url, token, origin) = (url.to_owned(), token.to_owned(), origin.map(str::to_owned));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Value, ClientError>>();
        let (stop_tx, stop_rx) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("fake-page".to_owned())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    let _ = ready_tx.send(Err(ClientError::Connect("no runtime".to_owned())));
                    return Value::Null;
                };
                rt.block_on(async move {
                    let opts = ConnectOpts { origin, ..ConnectOpts::default() };
                    let page = json!({ "version": env!("CARGO_PKG_VERSION"), "agent": "fake page" });
                    let hello = BridgeClient::connect_with_hello(&url, &opts, json!({ "token": token, "page": page })).await;
                    let mut client = match hello {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return Value::Null;
                        }
                    };
                    let _ = ready_tx.send(Ok(client.hello().clone()));
                    let mut session = make_session();
                    client.serve(|frame| handle_text(&mut session, frame), stop_rx).await;
                    session.state_json()
                })
            })
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        let hello = ready_rx.recv().map_err(|_| ClientError::Closed)??;
        Ok(Self { hello, stop: Some(stop_tx), thread: Some(thread) })
    }

    /// Connects as the hosted page (`Origin: https://drafft.ink`).
    pub fn connect_hosted(
        url: &str,
        token: &str,
        make_session: impl FnOnce() -> Session + Send + 'static,
    ) -> Result<Self, ClientError> {
        Self::connect(url, token, Some(HOSTED_ORIGIN), make_session)
    }

    /// The relay's answer to `hello`, including the `resume_token`.
    pub fn hello(&self) -> &Value {
        &self.hello
    }

    /// The resume token that the page keeps in its tab's `sessionStorage`.
    pub fn resume_token(&self) -> Option<&str> {
        self.hello.get("resume_token").and_then(Value::as_str)
    }

    /// Disconnects and returns the final `engine.state` of the page's session.
    pub fn disconnect(mut self) -> Value {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or(Value::Null)
    }
}

impl Drop for FakePage {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
