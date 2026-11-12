//! `drafftink-headless serve` as a process: it starts, answers `/health`, serves the source offer and stops. The
//! protocol, the stores and the hostile-input cases are tested where the server code is (`drafftink-server`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// These tests wait for a real process on a real socket, with the real clock.
#![allow(clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The server process; it is killed when the test ends, however it ends.
struct Server {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A port nothing listens on at this moment.
fn free_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
}

fn start(dir: &std::path::Path, extra: &[&str]) -> Server {
    let addr = free_addr();
    let child = Command::new(env!("CARGO_BIN_EXE_drafftink-headless"))
        .args(["serve", "--storage", "memory", "--bind", &addr.to_string(), "--web-dir"])
        .arg(dir)
        .args(extra)
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Server { child, addr }
}

/// One HTTP/1.1 request, and the whole response (status line, headers and body).
fn get(addr: SocketAddr, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

/// Waits for the server to answer, and fails if it exits first.
fn wait_until_up(server: &mut Server, path: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(response) = get(server.addr, path) {
            return response;
        }
        if let Some(status) = server.child.try_wait().unwrap() {
            panic!("the server exited before it answered: {status}");
        }
        assert!(Instant::now() < deadline, "the server did not answer on {}", server.addr);
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn serve_starts_on_a_free_port_and_answers_health() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = start(dir.path(), &[]);
    let response = wait_until_up(&mut server, "/health");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
}

#[test]
fn serve_serves_the_web_directory_and_the_source_offer_that_a_flag_sets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<title>web app</title>").unwrap();
    let mut server = start(dir.path(), &["--source-url", "https://git.example.org/me/fork"]);
    let page = wait_until_up(&mut server, "/");
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("<title>web app</title>"), "{page}");
    let source = get(server.addr, "/source.json").unwrap();
    assert!(source.contains("https://git.example.org/me/fork"), "{source}");
}

#[test]
fn serve_exits_with_2_for_arguments_that_make_no_configuration() {
    let out = Command::new(env!("CARGO_BIN_EXE_drafftink-headless"))
        .args(["serve", "--storage", "memory", "--max-peers-per-room", "0"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[cfg(unix)]
#[test]
fn serve_stops_cleanly_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = start(dir.path(), &[]);
    wait_until_up(&mut server, "/health");
    let signalled = Command::new("kill").args(["-TERM", &server.child.id().to_string()]).status().unwrap();
    assert!(signalled.success());
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = server.child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the server ignored SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status}");
}
