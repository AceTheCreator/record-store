//! Durable single-node metadata catalog.

use chrono::{DateTime, Duration, Utc};
use record_store_core::{
    Bucket, BucketId, DeleteMarker, ObjectId, ObjectKey, ObjectLockState, ObjectMetadata,
    ObjectVersionRecord, UploadId, UploadedPart, VersionId,
};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;

use crate::error::{backend, counter_error};
use crate::keys::{
    bucket_key, exact_version_prefix, lock_key, object_key, prefix_successor, version_order_key,
};
use crate::schema::{
    BUCKET_NAMES, BUCKET_USAGE, BUCKETS, CLEANUP, CLOCK, CLOCK_WATERMARK, COUNTERS, MARKERS,
    MULTIPART_ORDER, NULL_VERSIONS, OBJECT_LOCKS, OBJECTS, PARTS, VERSION_ORDER, VERSIONS,
};
use crate::types::BucketUsage;
use crate::*;

pub(crate) fn update_bucket_tx<F>(
    write: &redb::WriteTransaction,
    id: BucketId,
    update: F,
) -> Result<Bucket, MetadataError>
where
    F: FnOnce(&mut Bucket) -> Result<(), MetadataError>,
{
    let mut bucket = read_bucket(write, id)?.ok_or(MetadataError::BucketNotFound)?;
    update(&mut bucket)?;
    let bytes = serde_json::to_vec(&bucket)?;
    {
        let mut table = write
            .open_table(BUCKETS)
            .map_err(|e| backend("open buckets", e))?;
        table
            .insert(bucket_key(id).as_slice(), bytes.as_slice())
            .map_err(|e| backend("update bucket", e))?;
    }
    {
        let mut table = write
            .open_table(BUCKET_NAMES)
            .map_err(|e| backend("open bucket names", e))?;
        table
            .insert(bucket.name.as_str(), bytes.as_slice())
            .map_err(|e| backend("update bucket index", e))?;
    }
    Ok(bucket)
}

pub(crate) fn read_encoded<T: DeserializeOwned>(
    database: &Database,
    definition: TableDefinition<&[u8], &[u8]>,
    key: &[u8],
    operation: &'static str,
) -> Result<Option<T>, MetadataError> {
    let read = database
        .begin_read()
        .map_err(|e| backend("begin read", e))?;
    let table = read
        .open_table(definition)
        .map_err(|e| backend("open table", e))?;
    decode_optional(
        table
            .get(key)
            .map_err(|e| backend(operation, e))?
            .map(|v| v.value().to_vec()),
    )
}
pub(crate) fn read_tx<T: DeserializeOwned>(
    write: &redb::WriteTransaction,
    definition: TableDefinition<&[u8], &[u8]>,
    key: &[u8],
    operation: &'static str,
) -> Result<Option<T>, MetadataError> {
    let table = write
        .open_table(definition)
        .map_err(|e| backend("open table", e))?;
    decode_optional(
        table
            .get(key)
            .map_err(|e| backend(operation, e))?
            .map(|v| v.value().to_vec()),
    )
}
pub(crate) fn decode_optional<T: DeserializeOwned>(
    bytes: Option<Vec<u8>>,
) -> Result<Option<T>, MetadataError> {
    bytes
        .map(|value| serde_json::from_slice(&value))
        .transpose()
        .map_err(MetadataError::from)
}
pub(crate) fn read_bucket(
    write: &redb::WriteTransaction,
    id: BucketId,
) -> Result<Option<Bucket>, MetadataError> {
    read_tx(write, BUCKETS, &bucket_key(id), "read bucket")
}
pub(crate) fn read_bucket_usage(
    write: &redb::WriteTransaction,
    id: BucketId,
) -> Result<BucketUsage, MetadataError> {
    Ok(read_tx(write, BUCKET_USAGE, &bucket_key(id), "read bucket usage")?.unwrap_or_default())
}
pub(crate) fn write_bucket_usage(
    write: &redb::WriteTransaction,
    id: BucketId,
    usage: BucketUsage,
) -> Result<(), MetadataError> {
    let bytes = serde_json::to_vec(&usage)?;
    let mut table = write
        .open_table(BUCKET_USAGE)
        .map_err(|e| backend("open bucket usage", e))?;
    table
        .insert(bucket_key(id).as_slice(), bytes.as_slice())
        .map_err(|e| backend("write bucket usage", e))?;
    Ok(())
}

pub(crate) fn insert_version(
    write: &redb::WriteTransaction,
    record: &ObjectVersionRecord,
) -> Result<(), MetadataError> {
    let bytes = serde_json::to_vec(record)?;
    let id = record.version_id().as_uuid().as_bytes().to_vec();
    {
        let mut table = write
            .open_table(VERSIONS)
            .map_err(|e| backend("open versions", e))?;
        table
            .insert(id.as_slice(), bytes.as_slice())
            .map_err(|e| backend("insert version", e))?;
    }
    {
        let mut table = write
            .open_table(VERSION_ORDER)
            .map_err(|e| backend("open version order", e))?;
        table
            .insert(version_order_key(record).as_slice(), id.as_slice())
            .map_err(|e| backend("index version", e))?;
    }
    Ok(())
}
pub(crate) fn remove_version(
    write: &redb::WriteTransaction,
    record: &ObjectVersionRecord,
) -> Result<(), MetadataError> {
    {
        let mut table = write
            .open_table(VERSIONS)
            .map_err(|e| backend("open versions", e))?;
        table
            .remove(record.version_id().as_uuid().as_bytes().as_slice())
            .map_err(|e| backend("remove version", e))?;
    }
    {
        let mut table = write
            .open_table(VERSION_ORDER)
            .map_err(|e| backend("open version order", e))?;
        table
            .remove(version_order_key(record).as_slice())
            .map_err(|e| backend("remove version index", e))?;
    }
    Ok(())
}
pub(crate) fn take_null(
    write: &redb::WriteTransaction,
    bucket: BucketId,
    key: &ObjectKey,
) -> Result<Option<ObjectVersionRecord>, MetadataError> {
    let key = object_key(bucket, key);
    let id = {
        let mut table = write
            .open_table(NULL_VERSIONS)
            .map_err(|e| backend("open null versions", e))?;
        table
            .remove(key.as_slice())
            .map_err(|e| backend("remove null index", e))?
            .map(|v| v.value().to_vec())
    };
    id.map(|id| {
        read_tx(write, VERSIONS, &id, "read null version")?.ok_or_else(|| MetadataError::Database {
            operation: "read null version",
            reason: "inconsistent index".into(),
        })
    })
    .transpose()
}
pub(crate) fn set_null(
    write: &redb::WriteTransaction,
    key: &[u8],
    id: VersionId,
) -> Result<(), MetadataError> {
    let mut table = write
        .open_table(NULL_VERSIONS)
        .map_err(|e| backend("open null versions", e))?;
    table
        .insert(key, id.as_uuid().as_bytes().as_slice())
        .map_err(|e| backend("index null version", e))?;
    Ok(())
}
pub(crate) fn clear_null(
    write: &redb::WriteTransaction,
    key: &[u8],
    id: VersionId,
) -> Result<(), MetadataError> {
    let mut table = write
        .open_table(NULL_VERSIONS)
        .map_err(|e| backend("open null versions", e))?;
    let matches = table
        .get(key)
        .map_err(|e| backend("read null version", e))?
        .is_some_and(|v| v.value() == id.as_uuid().as_bytes().as_slice());
    if matches {
        table
            .remove(key)
            .map_err(|e| backend("remove null version", e))?;
    }
    Ok(())
}

pub(crate) fn latest_version(
    write: &redb::WriteTransaction,
    bucket: BucketId,
    key: &ObjectKey,
) -> Result<Option<ObjectVersionRecord>, MetadataError> {
    let prefix = exact_version_prefix(bucket, key);
    let end = prefix_successor(&prefix);
    let table = write
        .open_table(VERSION_ORDER)
        .map_err(|e| backend("open version order", e))?;
    let Some(entry) = table
        .range(prefix.as_slice()..end.as_slice())
        .map_err(|e| backend("range versions", e))?
        .next()
    else {
        return Ok(None);
    };
    let (_, id) = entry.map_err(|e| backend("read latest version", e))?;
    read_tx(write, VERSIONS, id.value(), "resolve latest version")
}
pub(crate) fn publish_current(
    write: &redb::WriteTransaction,
    key: &[u8],
    record: &ObjectVersionRecord,
) -> Result<(), MetadataError> {
    match record {
        ObjectVersionRecord::Object { metadata, .. } => {
            let bytes = serde_json::to_vec(metadata)?;
            let mut table = write
                .open_table(OBJECTS)
                .map_err(|e| backend("open objects", e))?;
            table
                .insert(key, bytes.as_slice())
                .map_err(|e| backend("publish current", e))?;
        }
        ObjectVersionRecord::DeleteMarker { marker, .. } => {
            let bytes = serde_json::to_vec(marker)?;
            let mut table = write
                .open_table(MARKERS)
                .map_err(|e| backend("open markers", e))?;
            table
                .insert(key, bytes.as_slice())
                .map_err(|e| backend("publish marker", e))?;
        }
    }
    Ok(())
}
pub(crate) fn remove_current(
    write: &redb::WriteTransaction,
    key: &[u8],
) -> Result<(), MetadataError> {
    {
        let mut table = write
            .open_table(OBJECTS)
            .map_err(|e| backend("open objects", e))?;
        table
            .remove(key)
            .map_err(|e| backend("remove current", e))?;
    }
    {
        let mut table = write
            .open_table(MARKERS)
            .map_err(|e| backend("open markers", e))?;
        table.remove(key).map_err(|e| backend("remove marker", e))?;
    }
    Ok(())
}
pub(crate) fn current_version(
    write: &redb::WriteTransaction,
    key: &[u8],
) -> Result<Option<VersionId>, MetadataError> {
    if let Some(metadata) = read_tx::<ObjectMetadata>(write, OBJECTS, key, "read current")? {
        return Ok(Some(metadata.version_id));
    }
    Ok(read_tx::<DeleteMarker>(write, MARKERS, key, "read marker")?.map(|m| m.version_id))
}
pub(crate) fn current_version_read(
    read: &redb::ReadTransaction,
    key: &[u8],
) -> Result<Option<VersionId>, MetadataError> {
    let objects = read
        .open_table(OBJECTS)
        .map_err(|e| backend("open objects", e))?;
    if let Some(bytes) = objects
        .get(key)
        .map_err(|e| backend("read current", e))?
        .map(|v| v.value().to_vec())
    {
        return Ok(Some(
            serde_json::from_slice::<ObjectMetadata>(&bytes)?.version_id,
        ));
    }
    let markers = read
        .open_table(MARKERS)
        .map_err(|e| backend("open markers", e))?;
    Ok(markers
        .get(key)
        .map_err(|e| backend("read marker", e))?
        .map(|v| serde_json::from_slice::<DeleteMarker>(v.value()))
        .transpose()?
        .map(|m| m.version_id))
}

pub(crate) fn list_parts_tx(
    write: &redb::WriteTransaction,
    id: UploadId,
) -> Result<Vec<UploadedPart>, MetadataError> {
    let table = write
        .open_table(PARTS)
        .map_err(|e| backend("open parts", e))?;
    let prefix = id.as_uuid().as_bytes().as_slice().to_vec();
    let end = prefix_successor(&prefix);
    let mut out = Vec::new();
    for entry in table
        .range(prefix.as_slice()..end.as_slice())
        .map_err(|e| backend("range parts", e))?
    {
        let (_, value) = entry.map_err(|e| backend("read part", e))?;
        out.push(serde_json::from_slice(value.value())?);
    }
    Ok(out)
}
pub(crate) fn has_multipart(
    write: &redb::WriteTransaction,
    bucket: BucketId,
) -> Result<bool, MetadataError> {
    let table = write
        .open_table(MULTIPART_ORDER)
        .map_err(|e| backend("open multipart order", e))?;
    let prefix = bucket_key(bucket);
    let end = prefix_successor(&prefix);
    Ok(table
        .range(prefix.as_slice()..end.as_slice())
        .map_err(|e| backend("range multipart", e))?
        .next()
        .is_some())
}
pub(crate) fn as_object(record: &ObjectVersionRecord) -> Option<&ObjectMetadata> {
    match record {
        ObjectVersionRecord::Object { metadata, .. } => Some(metadata),
        ObjectVersionRecord::DeleteMarker { .. } => None,
    }
}
pub(crate) fn record_matches(
    record: &ObjectVersionRecord,
    bucket: BucketId,
    key: &ObjectKey,
) -> bool {
    match record {
        ObjectVersionRecord::Object { metadata, .. } => {
            metadata.bucket_id == bucket && metadata.key == *key
        }
        ObjectVersionRecord::DeleteMarker { marker, .. } => {
            marker.bucket_id == bucket && marker.key == *key
        }
    }
}

pub(crate) fn queue_cleanup(
    write: &redb::WriteTransaction,
    id: ObjectId,
) -> Result<(), MetadataError> {
    let mut table = write
        .open_table(CLEANUP)
        .map_err(|e| backend("open cleanup", e))?;
    table
        .insert(id.as_uuid().as_bytes().as_slice(), &1)
        .map_err(|e| backend("queue cleanup", e))?;
    Ok(())
}
pub(crate) fn read_counter(
    table: &impl ReadableTable<&'static str, u64>,
    name: &'static str,
) -> Result<u64, MetadataError> {
    Ok(table
        .get(name)
        .map_err(|e| backend("read counter", e))?
        .map_or(0, |v| v.value()))
}
pub(crate) fn adjust_counter(
    write: &redb::WriteTransaction,
    name: &'static str,
    delta: impl Into<i128>,
) -> Result<(), MetadataError> {
    let mut table = write
        .open_table(COUNTERS)
        .map_err(|e| backend("open counters", e))?;
    let value = i128::from(read_counter(&table, name)?)
        .checked_add(delta.into())
        .and_then(|v| u64::try_from(v).ok())
        .ok_or_else(counter_error)?;
    table
        .insert(name, &value)
        .map_err(|e| backend("write counter", e))?;
    Ok(())
}

/// Reads the Object Lock state recorded for one version.
///
/// A version with no record is unlocked, which is why this returns a value
/// rather than an option: every caller would otherwise have to remember that
/// the absent case and the empty case mean the same thing.
pub(crate) fn read_object_lock(
    write: &redb::WriteTransaction,
    version: VersionId,
) -> Result<ObjectLockState, MetadataError> {
    Ok(read_tx(write, OBJECT_LOCKS, &lock_key(version), "read object lock")?.unwrap_or_default())
}

/// Persists Object Lock state, removing the record once nothing is held.
pub(crate) fn write_object_lock(
    write: &redb::WriteTransaction,
    version: VersionId,
    state: ObjectLockState,
) -> Result<(), MetadataError> {
    let key = lock_key(version);
    let mut table = write
        .open_table(OBJECT_LOCKS)
        .map_err(|e| backend("open object locks", e))?;
    if state.is_unlocked() {
        table
            .remove(key.as_slice())
            .map_err(|e| backend("clear object lock", e))?;
        return Ok(());
    }
    let bytes = serde_json::to_vec(&state)?;
    table
        .insert(key.as_slice(), bytes.as_slice())
        .map_err(|e| backend("write object lock", e))?;
    Ok(())
}

/// Drops the lock record of a version that is being removed.
pub(crate) fn remove_object_lock(
    write: &redb::WriteTransaction,
    version: VersionId,
) -> Result<(), MetadataError> {
    let key = lock_key(version);
    let mut table = write
        .open_table(OBJECT_LOCKS)
        .map_err(|e| backend("open object locks", e))?;
    table
        .remove(key.as_slice())
        .map_err(|e| backend("remove object lock", e))?;
    Ok(())
}

/// Inputs to any operation that could release a retained version.
///
/// Every value here is non-deterministic or configured, so it travels with the
/// command rather than being read from the environment during application. That
/// is what lets the same command sequence produce the same state anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockRelease {
    /// Wall-clock time as the caller observed it.
    pub observed_at: DateTime<Utc>,
    /// How far behind the high-water mark the clock may be before the catalog
    /// stops trusting it. This absorbs ordinary NTP correction, not a jump.
    pub clock_tolerance_seconds: u32,
    /// Whether the caller presented an authorized governance bypass.
    ///
    /// Only the lock service sets this, and only after checking the caller's
    /// explicit bypass permission. It never releases a compliance retention or
    /// a legal hold.
    pub bypass_governance: bool,
}

impl LockRelease {
    /// Builds the inputs for an operation that carries no bypass.
    #[must_use]
    pub const fn new(observed_at: DateTime<Utc>, clock_tolerance_seconds: u32) -> Self {
        Self {
            observed_at,
            clock_tolerance_seconds,
            bypass_governance: false,
        }
    }

    /// Returns the same inputs with an authorized governance bypass applied.
    #[must_use]
    pub const fn with_governance_bypass(mut self, bypass: bool) -> Self {
        self.bypass_governance = bypass;
        self
    }
}

/// Advances the observed-time high-water mark, or reports that it cannot be.
///
/// A clock that runs forward and comes back would otherwise let a retention
/// expire early and stay expired. The mark is never lowered, and the wall clock
/// is never substituted by the mark: a forward jump must not become a permanent
/// licence to delete, so the answer to a clock behind the mark is to refuse,
/// not to pick the larger number.
pub(crate) fn observe_clock(
    write: &redb::WriteTransaction,
    release: LockRelease,
) -> Result<(), MetadataError> {
    let now = release.observed_at.timestamp_micros();
    let mark = {
        let table = write
            .open_table(CLOCK)
            .map_err(|e| backend("open clock", e))?;
        table
            .get(CLOCK_WATERMARK)
            .map_err(|e| backend("read clock", e))?
            .map(|value| value.value())
    };
    if let Some(mark) = mark {
        let tolerance = Duration::seconds(i64::from(release.clock_tolerance_seconds))
            .num_microseconds()
            .unwrap_or(0);
        if now < mark.saturating_sub(tolerance) {
            return Err(MetadataError::ClockWentBackwards);
        }
    }
    if mark.is_none_or(|mark| now > mark) {
        let mut table = write
            .open_table(CLOCK)
            .map_err(|e| backend("open clock", e))?;
        table
            .insert(CLOCK_WATERMARK, &now)
            .map_err(|e| backend("write clock", e))?;
    }
    Ok(())
}

/// Refuses to remove a version that Object Lock still holds.
///
/// This runs inside the transaction that would do the removal, so a retention
/// placed or extended concurrently cannot slip between the check and the write.
pub(crate) fn enforce_deletable(
    write: &redb::WriteTransaction,
    version: VersionId,
    release: LockRelease,
) -> Result<(), MetadataError> {
    let state = read_object_lock(write, version)?;
    if state.is_unlocked() {
        return Ok(());
    }
    observe_clock(write, release)?;
    let Some(block) = state.deletion_block_at(release.observed_at) else {
        return Ok(());
    };
    if block.is_bypassable() && release.bypass_governance {
        return Ok(());
    }
    Err(MetadataError::VersionLocked(block))
}
