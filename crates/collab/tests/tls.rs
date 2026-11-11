#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `wss://` with rustls: a TLS-terminating proxy with a test certificate in front of the real server.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use common::live::*;
use drafftink_collab::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::{TcpListener, TcpStream};

const CERT: &[u8] = include_bytes!("fixtures/localhost.cert.der");
const KEY: &[u8] = include_bytes!("fixtures/localhost.key.der");

/// Terminates TLS on a loopback port and forwards the plain bytes to `target`. Runs until the process ends.
fn tls_proxy(target: SocketAddr) -> SocketAddr {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![CertificateDer::from(CERT.to_vec())],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())),
                )
                .unwrap();
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            loop {
                let Ok((socket, _)) = listener.accept().await else { continue };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(socket).await else { return };
                    let Ok(mut plain) = TcpStream::connect(target).await else { return };
                    let _ = tokio::io::copy_bidirectional(&mut tls, &mut plain).await;
                });
            }
        });
    });
    rx.recv_timeout(PATIENCE).unwrap()
}

fn trusting() -> NativeOptions {
    NativeOptions { tls: TlsOptions { extra_roots_der: vec![CERT.to_vec()] }, ..NativeOptions::default() }
}

#[test]
fn wss_works_with_a_trusted_certificate_and_is_refused_without_trust() {
    let server = Server::start(&["--auth-token", "tls-secret"]);
    let proxy = tls_proxy(server.addr);
    let url = format!("wss://localhost:{}", proxy.port());
    let mut w = World::new();
    let a = w.spawn(PeerSpec { token: Some("tls-secret"), options: trusting(), ..PeerSpec::new(1, &url, "tls") });
    let b = w.spawn(PeerSpec { token: Some("tls-secret"), options: trusting(), ..PeerSpec::new(2, &url, "tls") });
    // A client that was not told to trust the test certificate never gets as far as sending its token.
    let stranger = w.spawn(PeerSpec { token: Some("tls-secret"), ..PeerSpec::new(3, &url, "tls") });
    w.wait_until("trusted clients online", |w| {
        w.peers[a].state() == ConnState::Online && w.peers[b].state() == ConnState::Online
    });
    w.peers[a].add("over-tls");
    w.wait_until("B sees the edit through TLS", |w| w.peers[b].ids() == vec!["over-tls".to_owned()]);
    w.wait_until("the stranger has failed at least twice", |w| w.peers[stranger].client().stats().reconnects >= 2);
    let st = w.peers[stranger].client().status();
    assert_eq!(st.state, ConnState::Reconnecting, "{st:?}");
    assert_eq!(w.peers[stranger].client().stats().joins, 0);
    assert_eq!(w.peers[a].client().peers().len(), 1, "only B is in the room; the stranger never joined");
}

#[test]
fn wss_to_a_plain_ws_port_fails_without_sending_the_token() {
    let server = Server::start(&["--auth-token", "tls-secret"]);
    // wss:// pointed straight at the plain server: the TLS handshake cannot succeed.
    let url = format!("wss://localhost:{}", server.addr.port());
    let mut w = World::new();
    let a = w.spawn(PeerSpec { token: Some("tls-secret"), options: trusting(), ..PeerSpec::new(1, &url, "tls2") });
    w.wait_until("A fails to connect twice", |w| w.peers[a].client().stats().reconnects >= 2);
    assert_eq!(w.peers[a].client().stats().joins, 0);
}
