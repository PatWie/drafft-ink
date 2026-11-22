//! A stand-in for Cloudflare Access, for the parity run of the private environment.
//!
//! In production Access sits in front of the Worker: it checks the service token of a request (`CF-Access-Client-Id` and
//! `CF-Access-Client-Secret`), keeps those two headers to itself, and passes the request on with a signed
//! `Cf-Access-Jwt-Assertion`. The Worker checks that assertion against the key set of the Access team. Here the same
//! happens in two small servers on loopback:
//!
//! * the proxy, which the clients talk to: it answers a request with no valid token with `302` to a login page on a
//!   `*.cloudflareaccess.com` address (and never follows it), and passes a valid one to `wrangler dev` with the two
//!   headers removed and an assertion added whose `common_name` is the client id and which has no email, as a service
//!   token gets it;
//! * the key set server, which the Worker is configured to fetch (`ACCESS_CERTS_URL`).
//!
//! The assertions are signed (RS256) with the test key in `tests/fixtures/`. It signs nothing but these tests'
//! tokens, and nothing trusts it but a Worker that was told to.
//!
//! The proxy reads the head of each request, rewrites it, and then moves bytes both ways, so a WebSocket upgrade and a
//! picture upload pass through unchanged. A request that is not an upgrade is sent with `Connection: close`, so one
//! connection carries one request and the rewrite of the head is all there is to do.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use base64ct::{Base64UrlUnpadded, Encoding};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

/// The service token the stand-in accepts.
pub const CLIENT_ID: &str = "parity-test.access";
/// The secret of that token.
pub const CLIENT_SECRET: &str = "parity-test-secret-0123456789abcdef";
/// The Access team of the stand-in: the issuer of its assertions is `https://<TEAM>`.
pub const TEAM: &str = "parity.cloudflareaccess.com";
/// The audience tag of the Access application.
pub const AUD: &str = "parity-aud-tag";
/// The name the Worker believes is the application's hostname.
pub const APP_HOSTNAME: &str = "canvas.parity.test";

const KID: &str = "parity-key-1";
const TEST_KEY: &[u8] = include_bytes!("../fixtures/access-stand-in-test-key.der");
/// The longest request head the proxy reads.
const MAX_HEAD: usize = 64 * 1024;

/// What the proxy did with one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No valid token: answered with a redirect to the login page.
    Refused,
    /// Passed on, with these header names (lower case) and this `common_name` in the assertion.
    Forwarded { headers: Vec<String>, common_name: String },
}

/// The record of the proxy: one entry per request.
#[derive(Debug, Clone, Default)]
pub struct Log(Arc<Mutex<Vec<Verdict>>>);

impl Log {
    fn push(&self, verdict: Verdict) {
        lock(&self.0).push(verdict);
    }

    /// Everything the proxy decided so far.
    pub fn all(&self) -> Vec<Verdict> {
        lock(&self.0).clone()
    }

    /// How many requests were turned away.
    pub fn refused(&self) -> usize {
        self.all().iter().filter(|v| **v == Verdict::Refused).count()
    }

    /// The header names of every request that was passed on.
    pub fn forwarded(&self) -> Vec<Vec<String>> {
        self.all()
            .into_iter()
            .filter_map(|v| if let Verdict::Forwarded { headers, .. } = v { Some(headers) } else { None })
            .collect()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The signing key and the key set it publishes.
struct Signer {
    pair: RsaKeyPair,
}

impl Signer {
    fn new() -> Signer {
        Signer { pair: RsaKeyPair::from_der(TEST_KEY).expect("the committed test key is a DER RSAPrivateKey") }
    }

    /// An assertion for the service token `common_name`, valid for ten minutes.
    fn assertion(&self, common_name: &str) -> String {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("after 1970").as_secs();
        let header = serde_json::json!({ "alg": "RS256", "kid": KID, "typ": "JWT" });
        let claims = serde_json::json!({
            "iss": format!("https://{TEAM}"),
            "aud": [AUD],
            "iat": now,
            "nbf": now - 60,
            "exp": now + 600,
            "common_name": common_name,
            "type": "app",
        });
        let input = format!("{}.{}", b64(header.to_string().as_bytes()), b64(claims.to_string().as_bytes()));
        let mut signature = vec![0; self.pair.public().modulus_len()];
        self.pair.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), input.as_bytes(), &mut signature).expect("signs");
        format!("{input}.{}", b64(&signature))
    }

    /// The key set the Worker fetches: `{"keys": [...]}` with the public key as a JWK.
    fn key_set(&self) -> String {
        let (n, e) = rsa_public_numbers(self.pair.public().as_ref()).expect("a DER RSAPublicKey");
        serde_json::json!({ "keys": [{ "kty": "RSA", "kid": KID, "alg": "RS256", "use": "sig", "n": b64(n), "e": b64(e) }] })
            .to_string()
    }
}

fn b64(bytes: &[u8]) -> String {
    Base64UrlUnpadded::encode_string(bytes)
}

/// `(n, e)` of a DER `RSAPublicKey ::= SEQUENCE { INTEGER n, INTEGER e }`, without leading zero bytes.
fn rsa_public_numbers(der: &[u8]) -> Option<(&[u8], &[u8])> {
    fn take(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
        let (&first, rest) = input.split_first()?;
        if first != tag {
            return None;
        }
        let (&len, rest) = rest.split_first()?;
        let (len, rest) = if len < 0x80 {
            (usize::from(len), rest)
        } else {
            let count = usize::from(len & 0x7f);
            let (bytes, rest) = rest.split_at_checked(count)?;
            (bytes.iter().try_fold(0usize, |acc, b| acc.checked_mul(256)?.checked_add(usize::from(*b)))?, rest)
        };
        rest.split_at_checked(len)
    }
    let (body, _) = take(der, 0x30)?;
    let (n, rest) = take(body, 0x02)?;
    let (e, _) = take(rest, 0x02)?;
    Some((without_leading_zeros(n), without_leading_zeros(e)))
}

fn without_leading_zeros(x: &[u8]) -> &[u8] {
    let zeros = x.iter().take_while(|b| **b == 0).count();
    &x[zeros..]
}

/// The stand-in, running on a runtime of its own.
pub struct StandIn {
    runtime: Option<Runtime>,
    tasks: Vec<JoinHandle<()>>,
    /// Where the clients connect.
    pub front: SocketAddr,
    /// The address of the key set, for `ACCESS_CERTS_URL`.
    pub certs_url: String,
    /// What the proxy did.
    pub log: Log,
}

impl StandIn {
    /// Starts the key set server and the proxy in front of `upstream` (the `wrangler dev` address).
    pub fn start(upstream: SocketAddr) -> StandIn {
        let runtime =
            tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a runtime");
        let signer = Arc::new(Signer::new());
        let log = Log::default();
        let (certs, front) = runtime.block_on(async {
            let certs = TcpListener::bind("127.0.0.1:0").await.expect("a port for the key set");
            let front = TcpListener::bind("127.0.0.1:0").await.expect("a port for the proxy");
            (certs, front)
        });
        let certs_url = format!("http://{}/certs", certs.local_addr().expect("an address"));
        let front_addr = front.local_addr().expect("an address");
        let tasks = vec![
            runtime.spawn(serve_key_set(certs, signer.key_set())),
            runtime.spawn(serve_proxy(front, upstream, signer, log.clone())),
        ];
        StandIn { runtime: Some(runtime), tasks, front: front_addr, certs_url, log }
    }

    /// Stops both servers.
    pub fn stop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn serve_key_set(listener: TcpListener, body: String) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else { return };
        let body = body.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer).await;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn serve_proxy(listener: TcpListener, upstream: SocketAddr, signer: Arc<Signer>, log: Log) {
    loop {
        let Ok((client, _)) = listener.accept().await else { return };
        let (signer, log) = (Arc::clone(&signer), log.clone());
        tokio::spawn(async move {
            let _ = handle(client, upstream, &signer, &log).await;
        });
    }
}

/// A request head: the first line and the header lines.
struct Head {
    request_line: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn parse(text: &str) -> Option<Head> {
        let mut lines = text.split("\r\n");
        let request_line = lines.next()?.to_owned();
        let headers = lines
            .filter(|l| !l.is_empty())
            .map(|l| l.split_once(':').map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned())))
            .collect::<Option<Vec<_>>>()?;
        Some(Head { request_line, headers })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn is_upgrade(&self) -> bool {
        self.get("upgrade").is_some()
    }
}

/// The reply of Access to a request it does not let in: a redirect to its login page.
fn login_redirect() -> String {
    format!(
        "HTTP/1.1 302 Found\r\nlocation: https://{TEAM}/cdn-cgi/access/login/{APP_HOSTNAME}?kid={AUD}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    )
}

async fn handle(mut client: TcpStream, upstream: SocketAddr, signer: &Signer, log: &Log) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > MAX_HEAD {
            return Ok(());
        }
        let mut chunk = [0u8; 4096];
        let read = client.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let (head_bytes, rest) = buffer.split_at(end + 4);
    let Some(head) = Head::parse(&String::from_utf8_lossy(&head_bytes[..end])) else { return Ok(()) };

    let accepted = head.get("cf-access-client-id") == Some(CLIENT_ID)
        && head.get("cf-access-client-secret") == Some(CLIENT_SECRET);
    if !accepted {
        log.push(Verdict::Refused);
        client.write_all(login_redirect().as_bytes()).await?;
        return client.shutdown().await;
    }

    let upgrade = head.is_upgrade();
    let mut forwarded = format!("{}\r\n", head.request_line);
    let mut names = Vec::new();
    for (name, value) in &head.headers {
        let lower = name.to_ascii_lowercase();
        let drop = lower.starts_with("cf-access-") || (!upgrade && lower == "connection");
        if !drop {
            forwarded.push_str(&format!("{name}: {value}\r\n"));
            names.push(lower);
        }
    }
    forwarded.push_str(&format!("cf-access-jwt-assertion: {}\r\n", signer.assertion(CLIENT_ID)));
    names.push("cf-access-jwt-assertion".to_owned());
    if !upgrade {
        forwarded.push_str("connection: close\r\n");
        names.push("connection".to_owned());
    }
    forwarded.push_str("\r\n");
    log.push(Verdict::Forwarded { headers: names, common_name: CLIENT_ID.to_owned() });

    let mut server = TcpStream::connect(upstream).await?;
    server.write_all(forwarded.as_bytes()).await?;
    server.write_all(rest).await?;
    copy_bidirectional(&mut client, &mut server).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_key_is_read_from_der() {
        let signer = Signer::new();
        let (n, e) = rsa_public_numbers(signer.pair.public().as_ref()).expect("numbers");
        assert_eq!(n.len(), 256, "2048 bits");
        assert_eq!(e, [1, 0, 1]);
        assert!(n[0] != 0);
        assert!(rsa_public_numbers(&[]).is_none() && rsa_public_numbers(&[0x30, 0x80]).is_none());
        assert!(rsa_public_numbers(&[0x31, 0x00]).is_none());
    }

    #[test]
    fn an_assertion_has_three_parts_and_the_claims_of_a_service_token() {
        let token = Signer::new().assertion("abc.access");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value =
            serde_json::from_slice(&Base64UrlUnpadded::decode_vec(parts[1]).expect("base64url")).expect("json");
        assert_eq!(claims["common_name"], "abc.access");
        assert!(claims.get("email").is_none());
        assert_eq!(claims["iss"], format!("https://{TEAM}"));
    }

    #[test]
    fn a_head_is_read_and_a_broken_one_is_not() {
        let head = Head::parse("GET /r/x/ws HTTP/1.1\r\nHost: a\r\nUpgrade: websocket\r\n").expect("a head");
        assert_eq!(head.request_line, "GET /r/x/ws HTTP/1.1");
        assert_eq!(head.get("HOST"), Some("a"));
        assert!(head.is_upgrade());
        assert!(Head::parse("GET / HTTP/1.1\r\nno colon here\r\n").is_none());
    }
}
