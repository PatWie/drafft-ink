//! The S3 provider: objects in a bucket of any S3-compatible service (AWS S3, MinIO, Ceph,
//! Garage, R2, ...).
//!
//! Layout: `[<prefix>/]rooms/<room>/snapshot` and `[<prefix>/]rooms/<room>/updates/<n>`, where
//! `<n>` is a 20 digit log index. A PUT of one object is atomic, so a snapshot is replaced
//! whole, and covered log entries are deleted after the new snapshot is stored. A crash in
//! between leaves entries that the snapshot already contains, which Loro merges harmlessly.
//!
//! Writing one object per flush is why the server batches updates (the flush interval).

use std::sync::Arc;

use async_trait::async_trait;
use drafftink_protocol::RoomId;
use futures_util::{StreamExt, TryStreamExt};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};

use super::{NextIndex, RoomMeta, RoomStore, StoreError, StoredRoom, check_room};

/// How to reach the bucket. Credentials left out are looked up in the environment
/// (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, web identity, instance
/// metadata) by the object store client.
#[derive(Clone, Default)]
pub struct S3Config {
    /// Bucket name.
    pub bucket: String,
    /// Endpoint URL of an S3-compatible service; `None` means AWS.
    pub endpoint: Option<String>,
    /// Region; many S3-compatible services accept any value, `us-east-1` by convention.
    pub region: Option<String>,
    /// Access key id.
    pub access_key_id: Option<String>,
    /// Secret access key.
    pub secret_access_key: Option<String>,
    /// Session token for temporary credentials.
    pub session_token: Option<String>,
    /// Key prefix inside the bucket, without slashes at the ends.
    pub prefix: Option<String>,
    /// Allow a plain `http://` endpoint (local MinIO). Off by default.
    pub allow_http: bool,
    /// Use virtual-hosted-style addressing (`bucket.host`) instead of path style.
    pub virtual_hosted_style: bool,
    /// Delete objects one request at a time instead of with DeleteObjects, for services that
    /// lack the bulk call.
    pub disable_bulk_delete: bool,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id.as_ref().map(|_| "<set>"))
            .field("secret_access_key", &self.secret_access_key.as_ref().map(|_| "<set>"))
            .field("session_token", &self.session_token.as_ref().map(|_| "<set>"))
            .field("prefix", &self.prefix)
            .field("allow_http", &self.allow_http)
            .field("virtual_hosted_style", &self.virtual_hosted_style)
            .field("disable_bulk_delete", &self.disable_bulk_delete)
            .finish()
    }
}

/// Rooms as objects in a bucket.
pub struct S3Store {
    store: Arc<dyn ObjectStore>,
    prefix: Vec<String>,
    next: NextIndex,
}

fn backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("s3: {e}"))
}

impl S3Store {
    /// Builds the client. No request is made until the first use.
    pub fn new(config: &S3Config) -> Result<Self, StoreError> {
        if config.bucket.is_empty() {
            return Err(StoreError::Backend("s3: the bucket name is empty".into()));
        }
        let mut builder = AmazonS3Builder::from_env()
            .with_bucket_name(&config.bucket)
            .with_allow_http(config.allow_http)
            .with_virtual_hosted_style_request(config.virtual_hosted_style)
            .with_disable_bulk_delete(config.disable_bulk_delete);
        if let Some(v) = &config.endpoint {
            builder = builder.with_endpoint(v);
        }
        builder = builder.with_region(config.region.clone().unwrap_or_else(|| "us-east-1".into()));
        if let Some(v) = &config.access_key_id {
            builder = builder.with_access_key_id(v);
        }
        if let Some(v) = &config.secret_access_key {
            builder = builder.with_secret_access_key(v);
        }
        if let Some(v) = &config.session_token {
            builder = builder.with_token(v);
        }
        let store = builder.build().map_err(backend)?;
        let prefix = config
            .prefix
            .as_deref()
            .unwrap_or_default()
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(S3Store { store: Arc::new(store), prefix, next: NextIndex::default() })
    }

    fn path<'a>(&'a self, tail: impl IntoIterator<Item = &'a str>) -> Path {
        Path::from_iter(self.prefix.iter().map(String::as_str).chain(tail))
    }

    fn room_prefix(&self, room: &RoomId) -> Path {
        self.path(["rooms", room.as_str()])
    }

    fn snapshot_path(&self, room: &RoomId) -> Path {
        self.path(["rooms", room.as_str(), "snapshot"])
    }

    fn update_path(&self, room: &RoomId, n: u64) -> Path {
        self.path(["rooms", room.as_str(), "updates", &format!("{n:020}")])
    }

    /// The log entries of a room, oldest first: `(index, path)`.
    async fn log_entries(&self, room: &RoomId) -> Result<Vec<(u64, Path)>, StoreError> {
        let prefix = self.path(["rooms", room.as_str(), "updates"]);
        let metas: Vec<_> = self.store.list(Some(&prefix)).try_collect().await.map_err(backend)?;
        let mut out: Vec<(u64, Path)> = metas
            .into_iter()
            .filter_map(|m| {
                let n = m.location.filename()?.parse::<u64>().ok()?;
                Some((n, m.location))
            })
            .collect();
        out.sort();
        Ok(out)
    }

    async fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
        match self.store.get(path).await {
            Ok(got) => Ok(Some(got.bytes().await.map_err(backend)?.to_vec())),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    /// Deletes objects, in bulk where the service supports it. Missing objects are fine.
    async fn delete_paths(&self, paths: Vec<Path>) -> Result<(), StoreError> {
        if paths.is_empty() {
            return Ok(());
        }
        let input = futures_util::stream::iter(paths.into_iter().map(Ok)).boxed();
        let mut results = self.store.delete_stream(input);
        while let Some(result) = results.next().await {
            match result {
                Ok(_) | Err(object_store::Error::NotFound { .. }) => {}
                Err(e) => return Err(backend(e)),
            }
        }
        Ok(())
    }
}

#[async_trait]
impl RoomStore for S3Store {
    fn name(&self) -> &'static str {
        "s3"
    }

    async fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError> {
        check_room(room)?;
        let snapshot = self.read(&self.snapshot_path(room)).await?;
        let entries = self.log_entries(room).await?;
        let mut updates = Vec::with_capacity(entries.len());
        for (_, path) in &entries {
            // A concurrent compaction by another server would delete entries; that setup is
            // unsupported, so a missing entry is damage.
            let bytes = self
                .read(path)
                .await?
                .ok_or_else(|| StoreError::Corrupt(format!("log entry {path} vanished while loading")))?;
            updates.push(bytes);
        }
        self.next.set(room, entries.last().map_or(0, |(n, _)| n + 1));
        if snapshot.is_none() && updates.is_empty() {
            return Ok(None);
        }
        Ok(Some(StoredRoom { snapshot, updates }))
    }

    async fn append_update(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError> {
        check_room(room)?;
        let next = match self.next.get(room) {
            Some(n) => n,
            None => self.log_entries(room).await?.last().map_or(0, |(n, _)| n + 1),
        };
        self.store.put(&self.update_path(room, next), PutPayload::from(update.to_vec())).await.map_err(backend)?;
        self.next.set(room, next + 1);
        Ok(())
    }

    async fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError> {
        check_room(room)?;
        let entries = self.log_entries(room).await?;
        self.store.put(&self.snapshot_path(room), PutPayload::from(snapshot.to_vec())).await.map_err(backend)?;
        let drop_n = usize::try_from(covered).unwrap_or(usize::MAX);
        self.delete_paths(entries.into_iter().take(drop_n).map(|(_, p)| p).collect()).await
    }

    async fn delete(&self, room: &RoomId) -> Result<(), StoreError> {
        check_room(room)?;
        self.next.forget(room);
        let metas: Vec<_> = self.store.list(Some(&self.room_prefix(room))).try_collect().await.map_err(backend)?;
        self.delete_paths(metas.into_iter().map(|m| m.location).collect()).await
    }

    async fn list(&self) -> Result<Vec<RoomMeta>, StoreError> {
        let root = self.path(["rooms"]);
        let skip = self.prefix.len() + 1;
        let metas: Vec<_> = self.store.list(Some(&root)).try_collect().await.map_err(backend)?;
        let mut rooms: std::collections::BTreeMap<RoomId, u64> = std::collections::BTreeMap::new();
        for meta in metas {
            let Some(part) = meta.location.parts().nth(skip) else { continue };
            let Ok(room) = RoomId::new(part.as_ref().to_owned()) else { continue };
            let ms = u64::try_from(meta.last_modified.timestamp_millis()).unwrap_or(0);
            let slot = rooms.entry(room).or_insert(0);
            *slot = (*slot).max(ms);
        }
        Ok(rooms.into_iter().map(|(room, modified_ms)| RoomMeta { room, modified_ms }).collect())
    }
}
