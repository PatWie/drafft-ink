//! An in-process S3-compatible server for the conformance tests when no real S3 endpoint
//! (MinIO, RustFS, ...) is configured. It implements path-style PUT, GET, HEAD, DELETE and
//! ListObjectsV2 with prefix, delimiter and continuation tokens (three keys per page, so
//! pagination is exercised). Requests must carry an AWS Signature V4 `Authorization` header;
//! the signature itself is not verified.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use drafftink_server::clock::now_ms;

const PAGE: usize = 3;

#[derive(Default)]
pub struct Objects {
    map: BTreeMap<String, (Vec<u8>, u64)>,
}

type Shared = Arc<Mutex<Objects>>;

pub struct MockS3 {
    pub addr: SocketAddr,
    pub objects: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockS3 {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockS3 {
    pub async fn start() -> MockS3 {
        let objects: Shared = Arc::default();
        let app = Router::new()
            .route("/{bucket}", get(list).post(bulk_delete))
            .route("/{bucket}/", get(list).post(bulk_delete))
            .route("/{bucket}/{*key}", get(get_object).head(head_object).put(put_object).delete(delete_object))
            .layer(DefaultBodyLimit::disable())
            .with_state(objects.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        MockS3 { addr, objects, task }
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn keys(&self) -> Vec<String> {
        self.objects.lock().expect("lock").map.keys().cloned().collect()
    }
}

fn signed(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 ") && v.contains("Credential=") && v.contains("Signature="))
}

fn xml_error(status: StatusCode, code: &str) -> Response {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>{code}</Message></Error>"
    );
    (status, [(header::CONTENT_TYPE, "application/xml")], body).into_response()
}

async fn put_object(
    State(s): State<Shared>,
    Path((_bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !signed(&headers) {
        return xml_error(StatusCode::FORBIDDEN, "AccessDenied");
    }
    s.lock().expect("lock").map.insert(key, (body.to_vec(), now_ms()));
    (StatusCode::OK, [(header::ETAG, "\"mock-etag\"")]).into_response()
}

async fn get_object(
    State(s): State<Shared>,
    Path((_bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !signed(&headers) {
        return xml_error(StatusCode::FORBIDDEN, "AccessDenied");
    }
    match s.lock().expect("lock").map.get(&key) {
        Some((data, ms)) => (
            StatusCode::OK,
            [
                (header::ETAG, "\"mock-etag\"".to_owned()),
                (header::LAST_MODIFIED, http_date(*ms)),
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            ],
            data.clone(),
        )
            .into_response(),
        None => xml_error(StatusCode::NOT_FOUND, "NoSuchKey"),
    }
}

async fn head_object(
    State(s): State<Shared>,
    Path((_bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !signed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match s.lock().expect("lock").map.get(&key) {
        Some((data, ms)) => (
            StatusCode::OK,
            [
                (header::ETAG, "\"mock-etag\"".to_owned()),
                (header::LAST_MODIFIED, http_date(*ms)),
                (header::CONTENT_LENGTH, data.len().to_string()),
            ],
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn delete_object(
    State(s): State<Shared>,
    Path((_bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !signed(&headers) {
        return xml_error(StatusCode::FORBIDDEN, "AccessDenied");
    }
    s.lock().expect("lock").map.remove(&key);
    StatusCode::NO_CONTENT.into_response()
}

/// DeleteObjects: `POST /bucket?delete` with `<Delete><Object><Key>..</Key></Object>..</Delete>`.
async fn bulk_delete(
    State(s): State<Shared>,
    Path(_bucket): Path<String>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !signed(&headers) {
        return xml_error(StatusCode::FORBIDDEN, "AccessDenied");
    }
    if !q.contains_key("delete") {
        return xml_error(StatusCode::BAD_REQUEST, "InvalidRequest");
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><DeleteResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
    );
    let mut guard = s.lock().expect("lock");
    for part in text.split("<Key>").skip(1) {
        let Some(key) = part.split("</Key>").next() else { continue };
        guard.map.remove(key);
        xml.push_str(&format!("<Deleted><Key>{key}</Key></Deleted>"));
    }
    xml.push_str("</DeleteResult>");
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/xml")], xml).into_response()
}

async fn list(
    State(s): State<Shared>,
    Path(_bucket): Path<String>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !signed(&headers) {
        return xml_error(StatusCode::FORBIDDEN, "AccessDenied");
    }
    let prefix = q.get("prefix").cloned().unwrap_or_default();
    let delimiter = q.get("delimiter").cloned().filter(|d| !d.is_empty());
    let after = q.get("continuation-token").cloned().or_else(|| q.get("start-after").cloned()).unwrap_or_default();
    let guard = s.lock().expect("lock");
    let mut contents = Vec::new();
    let mut prefixes = std::collections::BTreeSet::new();
    let mut truncated = false;
    let mut last = String::new();
    for (key, (data, ms)) in guard.map.range(after.clone()..) {
        if *key == after || !key.starts_with(&prefix) {
            continue;
        }
        if contents.len() + prefixes.len() >= PAGE {
            truncated = true;
            break;
        }
        last = key.clone();
        if let Some(d) = &delimiter
            && let Some(i) = key[prefix.len()..].find(d.as_str())
        {
            prefixes.insert(key[..prefix.len() + i + d.len()].to_owned());
            continue;
        }
        contents.push((key.clone(), data.len(), *ms));
    }
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
    );
    xml.push_str(&format!(
        "<Name>bucket</Name><Prefix>{prefix}</Prefix><KeyCount>{}</KeyCount><MaxKeys>{PAGE}</MaxKeys>",
        contents.len() + prefixes.len()
    ));
    xml.push_str(&format!("<IsTruncated>{truncated}</IsTruncated>"));
    if truncated {
        xml.push_str(&format!("<NextContinuationToken>{last}</NextContinuationToken>"));
    }
    for (key, size, ms) in contents {
        xml.push_str(&format!(
            "<Contents><Key>{key}</Key><LastModified>{}</LastModified><ETag>&quot;mock-etag&quot;</ETag><Size>{size}</Size><StorageClass>STANDARD</StorageClass></Contents>",
            iso8601(ms)
        ));
    }
    for p in prefixes {
        xml.push_str(&format!("<CommonPrefixes><Prefix>{p}</Prefix></CommonPrefixes>"));
    }
    xml.push_str("</ListBucketResult>");
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/xml")], xml).into_response()
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn iso8601(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, rem % 3600 / 60, rem % 60, ms % 1000)
}

fn http_date(ms: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let (y, m, d) = civil(days);
    let rem = secs.rem_euclid(86_400);
    format!(
        "{}, {d:02} {} {y:04} {:02}:{:02}:{:02} GMT",
        DAYS[days.rem_euclid(7) as usize],
        MONTHS[(m - 1) as usize],
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
