//! The JavaScript objects that do the SQL (`src/sql-store.ts`, `src/blobs.ts`), seen as [`SqlHost`] and
//! [`BlobHost`]. The calls are synchronous: `ctx.storage.sql.exec` is.
//!
//! A byte slice goes to JavaScript as a view of the module's memory, which is valid only during the call. The shell
//! binds it to a statement at once, which copies it, and never keeps it.

use drafftink_protocol::v2::{IMAGE_NAME_BYTES, ImageName};
use drafftink_relay::BlobStats;
use js_sys::{Array, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use super::{ms, num};
use crate::host::{BlobHost, HostError, SqlHost};

#[wasm_bindgen(typescript_custom_section)]
const SQL_HOST_TS: &str = r#"
/** The rows of one room. Every call is synchronous. A failure is thrown as an object with `kind` ("daily_limit" or "storage") and `message`. */
export interface SqlHost {
  loadMeta(): Uint8Array | null;
  saveMeta(meta: Uint8Array): void;
  /** One transaction: the segment row and the meta. */
  appendSegment(first: number, last: number, data: Uint8Array, meta: Uint8Array): void;
  /** Segments whose last record is above `afterSeq`, oldest first, until `maxBytes` (0: no limit) of data; at least one if any. */
  readSegments(afterSeq: number, maxBytes: number): Uint8Array[];
  /** `[covers, sealed record]` or null. */
  readSnapshot(): [number, Uint8Array] | null;
  /** One transaction: the snapshot, the meta, and the deletion of segments with `last <= covers`. */
  putSnapshot(covers: number, record: Uint8Array, meta: Uint8Array): void;
  /** The shell wipes the database when the core reports the deletion; nothing to do here. */
  deleteAll(): void;
}
/** The sealed pictures of one room. Every call is synchronous. */
export interface BlobHost {
  put(name: Uint8Array, sealed: Uint8Array): void;
  get(name: Uint8Array): Uint8Array | null;
  head(name: Uint8Array): number | null;
  /** `[name, size]` pairs. */
  list(): [Uint8Array, number][];
  /** `[count, bytes]`. */
  stats(): [number, number];
  deleteAll(): void;
}
"#;

#[wasm_bindgen]
extern "C" {
    /// The `SqlHost` object of the shell.
    #[wasm_bindgen(typescript_type = "SqlHost")]
    pub type SqlHostJs;

    #[wasm_bindgen(method, catch, js_name = loadMeta)]
    fn load_meta(this: &SqlHostJs) -> Result<Option<Vec<u8>>, JsValue>;
    #[wasm_bindgen(method, catch, js_name = saveMeta)]
    fn save_meta(this: &SqlHostJs, meta: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = appendSegment)]
    fn append_segment(this: &SqlHostJs, first: f64, last: f64, data: &[u8], meta: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = readSegments)]
    fn read_segments(this: &SqlHostJs, after_seq: f64, max_bytes: f64) -> Result<Array, JsValue>;
    #[wasm_bindgen(method, catch, js_name = readSnapshot)]
    fn read_snapshot(this: &SqlHostJs) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch, js_name = putSnapshot)]
    fn put_snapshot(this: &SqlHostJs, covers: f64, record: &[u8], meta: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = deleteAll)]
    fn delete_all(this: &SqlHostJs) -> Result<(), JsValue>;

    /// The `BlobHost` object of the shell.
    #[wasm_bindgen(typescript_type = "BlobHost")]
    pub type BlobHostJs;

    #[wasm_bindgen(method, catch, js_name = put)]
    fn blob_put(this: &BlobHostJs, name: &[u8], sealed: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = get)]
    fn blob_get(this: &BlobHostJs, name: &[u8]) -> Result<Option<Vec<u8>>, JsValue>;
    #[wasm_bindgen(method, catch, js_name = head)]
    fn blob_head(this: &BlobHostJs, name: &[u8]) -> Result<Option<f64>, JsValue>;
    #[wasm_bindgen(method, catch, js_name = list)]
    fn blob_list(this: &BlobHostJs) -> Result<Array, JsValue>;
    #[wasm_bindgen(method, catch, js_name = stats)]
    fn blob_stats(this: &BlobHostJs) -> Result<Array, JsValue>;
    #[wasm_bindgen(method, catch, js_name = deleteAll)]
    fn blob_delete_all(this: &BlobHostJs) -> Result<(), JsValue>;
}

/// What an exception of the shell means: `kind` says whether the daily limit is used up.
fn host_error(thrown: JsValue) -> HostError {
    let field = |name: &str| Reflect::get(&thrown, &JsValue::from_str(name)).ok().and_then(|v| v.as_string());
    if field("kind").as_deref() == Some("daily_limit") {
        return HostError::DailyLimit;
    }
    HostError::Failed(field("message").unwrap_or_else(|| "the storage of the host failed".to_owned()))
}

fn damaged(what: &str) -> HostError {
    HostError::Failed(format!("the host returned {what} in an unexpected shape"))
}

fn bytes_of(value: &JsValue) -> Option<Vec<u8>> {
    value.dyn_ref::<Uint8Array>().map(Uint8Array::to_vec)
}

fn seq_of(value: &JsValue) -> Option<u64> {
    value.as_f64().map(ms)
}

/// The SQL of the shell.
pub struct JsSql(SqlHostJs);

impl JsSql {
    pub fn new(host: SqlHostJs) -> Self {
        JsSql(host)
    }
}

impl SqlHost for JsSql {
    fn load_meta(&mut self) -> Result<Option<Vec<u8>>, HostError> {
        self.0.load_meta().map_err(host_error)
    }

    fn save_meta(&mut self, meta: &[u8]) -> Result<(), HostError> {
        self.0.save_meta(meta).map_err(host_error)
    }

    fn append_segment(&mut self, first: u64, last: u64, data: &[u8], meta: &[u8]) -> Result<(), HostError> {
        self.0.append_segment(num(first), num(last), data, meta).map_err(host_error)
    }

    fn read_segments(&mut self, after_seq: u64, max_bytes: usize) -> Result<Vec<Vec<u8>>, HostError> {
        let rows = self.0.read_segments(num(after_seq), max_bytes as f64).map_err(host_error)?;
        rows.iter().map(|row| bytes_of(&row).ok_or_else(|| damaged("a segment"))).collect()
    }

    fn read_snapshot(&mut self) -> Result<Option<(u64, Vec<u8>)>, HostError> {
        let value = self.0.read_snapshot().map_err(host_error)?;
        if value.is_null() || value.is_undefined() {
            return Ok(None);
        }
        let pair = Array::from(&value);
        let covers = seq_of(&pair.get(0)).ok_or_else(|| damaged("a snapshot"))?;
        let record = bytes_of(&pair.get(1)).ok_or_else(|| damaged("a snapshot"))?;
        Ok(Some((covers, record)))
    }

    fn put_snapshot(&mut self, covers: u64, record: &[u8], meta: &[u8]) -> Result<(), HostError> {
        self.0.put_snapshot(num(covers), record, meta).map_err(host_error)
    }

    fn delete_all(&mut self) -> Result<(), HostError> {
        self.0.delete_all().map_err(host_error)
    }
}

/// The pictures of the shell.
pub struct JsBlobs(BlobHostJs);

impl JsBlobs {
    pub fn new(host: BlobHostJs) -> Self {
        JsBlobs(host)
    }
}

fn name_of(value: &JsValue) -> Option<ImageName> {
    let bytes: [u8; IMAGE_NAME_BYTES] = bytes_of(value)?.try_into().ok()?;
    Some(ImageName::from_bytes(bytes))
}

impl BlobHost for JsBlobs {
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), HostError> {
        self.0.blob_put(name.as_bytes(), sealed).map_err(host_error)
    }

    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, HostError> {
        self.0.blob_get(name.as_bytes()).map_err(host_error)
    }

    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, HostError> {
        Ok(self.0.blob_head(name.as_bytes()).map_err(host_error)?.map(ms))
    }

    fn list(&mut self) -> Result<Vec<(ImageName, u64)>, HostError> {
        let rows = self.0.blob_list().map_err(host_error)?;
        rows.iter()
            .map(|row| {
                let pair = Array::from(&row);
                Some((name_of(&pair.get(0))?, seq_of(&pair.get(1))?))
            })
            .map(|entry| entry.ok_or_else(|| damaged("a picture list")))
            .collect()
    }

    fn stats(&mut self) -> Result<BlobStats, HostError> {
        let pair = self.0.blob_stats().map_err(host_error)?;
        let count = seq_of(&pair.get(0)).and_then(|n| u32::try_from(n).ok());
        let bytes = seq_of(&pair.get(1));
        match (count, bytes) {
            (Some(count), Some(bytes)) => Ok(BlobStats { count, bytes }),
            _ => Err(damaged("picture totals")),
        }
    }

    fn delete_all(&mut self) -> Result<(), HostError> {
        self.0.blob_delete_all().map_err(host_error)
    }
}
