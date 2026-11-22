//! A small blocking HTTP client for tests that call a relay's picture routes by hand: any method, any headers, and the
//! answer whatever its status is (a refusal is a result here, not an error).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::time::Duration;

/// What a relay answered.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One request to `http://addr<path>`. A transport failure panics with the cause: the relay should be there.
pub fn request(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, String)], body: &[u8]) -> Reply {
    try_request(addr, method, path, headers, body).unwrap_or_else(|e| panic!("{method} {path} on {addr} failed: {e}"))
}

/// Like [`request`], with the failure as a value.
pub fn try_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> Result<Reply, ureq::Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let url = format!("http://{addr}{path}");
    let mut builder = ureq::http::Request::builder().method(method).uri(url);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    let request = builder.body(body.to_vec()).map_err(ureq::Error::from)?;
    let mut response = agent.run(request)?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_vec()?;
    Ok(Reply { status, body })
}
