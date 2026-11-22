//! Mock servers on loopback ports that record what they receive: a WebSocket server (answers the handshake as told) and
//! a plain HTTP server (answers every request the same way). For the tests of the service token on the wire.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use drafftink_collab::ServerUrl;
use drafftink_collab::UrlPolicy;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;

/// What a mock server saw of one request.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub first_line: String,
    pub headers: HashMap<String, String>,
}

impl Seen {
    pub fn carries_token(&self) -> bool {
        self.headers.contains_key("cf-access-client-id") || self.headers.contains_key("cf-access-client-secret")
    }
}

pub type Log = Arc<Mutex<Vec<Seen>>>;

pub fn seen(log: &Log) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

// ---- the WebSocket handshake ----------------------------------------------------------------------------------

/// What a mock WebSocket server answers to the handshake.
#[derive(Clone)]
pub enum Handshake {
    /// Accept it.
    Accept,
    /// Refuse it with this status, headers and body.
    Refuse { status: u16, location: Option<String>, body: &'static str },
}

pub fn refuse(status: u16, location: Option<&str>, body: &'static str) -> Handshake {
    Handshake::Refuse { status, location: location.map(str::to_owned), body }
}

/// A WebSocket server on a loopback port that records the headers of each handshake.
pub struct MockSocket {
    pub addr: SocketAddr,
    pub log: Log,
}

impl MockSocket {
    pub fn start(answer: Handshake) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Log::default();
        let seen = Arc::clone(&log);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (seen, answer) = (Arc::clone(&seen), answer.clone());
                thread::spawn(move || serve_handshake(stream, &seen, &answer));
            }
        });
        MockSocket { addr, log }
    }

    pub fn url(&self) -> ServerUrl {
        ServerUrl::parse(&format!("ws://{}/ws", self.addr), UrlPolicy::default()).unwrap()
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }
}

fn serve_handshake(stream: TcpStream, log: &Log, answer: &Handshake) {
    let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let headers = request.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()));
        log.lock().unwrap().push(Seen { first_line: request.uri().to_string(), headers: headers.collect() });
        match answer {
            Handshake::Accept => Ok(response),
            Handshake::Refuse { status, location, body } => {
                let mut builder = http::Response::builder().status(*status);
                if let Some(location) = location {
                    builder = builder.header("location", location);
                }
                Err(builder.body(Some((*body).to_owned())).unwrap())
            }
        }
    };
    // A refused handshake ends in an error here, which is the point; an accepted one only needs to stay open.
    if let Ok(mut socket) = tokio_tungstenite::tungstenite::accept_hdr(stream, callback) {
        while socket.read().is_ok() {}
    }
}


/// A plain HTTP server on a loopback port that records each request and answers every one the same way.
pub struct MockHttp {
    pub addr: SocketAddr,
    pub log: Log,
    pub connections: Arc<AtomicUsize>,
}

impl MockHttp {
    pub fn start(status: u16, location: Option<String>, body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Log::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let (seen, count) = (Arc::clone(&log), Arc::clone(&connections));
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                count.fetch_add(1, Ordering::SeqCst);
                let (seen, location) = (Arc::clone(&seen), location.clone());
                thread::spawn(move || serve_http(stream, &seen, status, location.as_deref(), body));
            }
        });
        MockHttp { addr, log, connections }
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

fn serve_http(mut stream: TcpStream, log: &Log, status: u16, location: Option<&str>, body: &str) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let mut request = Seen { first_line: lines.next().unwrap_or_default().to_owned(), ..Seen::default() };
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            request.headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    // Take a request body off the wire, so the client is not answered mid-write.
    let length: usize = request.headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut rest = vec![0u8; length];
    let _ = stream.read_exact(&mut rest);
    log.lock().unwrap().push(request);
    let location = location.map(|l| format!("Location: {l}\r\n")).unwrap_or_default();
    let reply = format!(
        "HTTP/1.1 {status} X\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

