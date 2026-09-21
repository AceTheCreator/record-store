//! The storage event a committed mutation owes its subscribers.
//!
//! An event published after a mutation commits can be lost: the process can die
//! in the window between the two, and the publish itself can fail. Either way
//! the change happened and nobody was told, which for an integration driving
//! downstream work is indistinguishable from the change never happening.
//!
//! So the *intent* to publish is written inside the transaction that commits
//! the mutation, in the catalog that is authoritative for it. A crash anywhere
//! after that leaves a durable row describing an event that is owed, and
//! delivery resumes from it. The identifier is allocated there too, so a
//! republish after a crash is the same event rather than a second one.
//!
//! These types live here, below both the catalog that writes the row and the
//! outbox that drains it, so neither has to depend on the other.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{EventId, VersionId};

/// Stable storage-event names intended for integrations.
///
/// Part of the published webhook contract, so the serialized spelling of each
/// variant is fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageEventType {
    #[serde(rename = "bucket.created")]
    BucketCreated,
    #[serde(rename = "bucket.deleted")]
    BucketDeleted,
    #[serde(rename = "object.created")]
    ObjectCreated,
    #[serde(rename = "object.updated")]
    ObjectUpdated,
    #[serde(rename = "object.deleted")]
    ObjectDeleted,
    #[serde(rename = "object.restored")]
    ObjectRestored,
    #[serde(rename = "multipart.completed")]
    MultipartCompleted,
    #[serde(rename = "multipart.aborted")]
    MultipartAborted,
}

impl StorageEventType {
    /// Returns the wire name carried in the delivery header.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BucketCreated => "bucket.created",
            Self::BucketDeleted => "bucket.deleted",
            Self::ObjectCreated => "object.created",
            Self::ObjectUpdated => "object.updated",
            Self::ObjectDeleted => "object.deleted",
            Self::ObjectRestored => "object.restored",
            Self::MultipartCompleted => "multipart.completed",
            Self::MultipartAborted => "multipart.aborted",
        }
    }
}

/// Why a version was written.
///
/// The catalog cannot tell a copy from a restore from the assembly of a
/// multipart upload: all three publish a version and look identical once they
/// arrive. The caller knows, so it says, and the event a subscriber receives
/// keeps describing what actually happened.
///
/// Carried on the command rather than inferred, for the same reason every other
/// non-deterministic input is: the command has to mean the same thing wherever
/// it is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOrigin {
    /// An ordinary upload.
    #[default]
    Direct,
    /// A server-side copy.
    Copy,
    /// The object a multipart upload assembled.
    MultipartCompletion,
    /// A historical version promoted back to current.
    Restore,
}

/// One storage event a committed mutation owes, as recorded in the catalog.
///
/// The `sequence` is allocated inside the committing transaction, so the order
/// of these rows is commit order — which is the order subscribers see, and the
/// order a resumed drain continues from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationEvent {
    /// Position in commit order.
    pub sequence: u64,
    /// The identifier the event will carry, fixed at commit time.
    ///
    /// Allocated here rather than at publish time so that a drain interrupted
    /// and resumed republishes the same event instead of inventing a second
    /// one with the same content.
    pub event_id: EventId,
    /// What happened.
    pub event_type: StorageEventType,
    /// When the mutation committed.
    pub occurred_at: DateTime<Utc>,
    /// Bucket name, as a subscriber knows it.
    pub bucket: String,
    /// Object key, for events that name one.
    pub key: Option<String>,
    /// Version the event refers to, when it refers to one.
    pub version_id: Option<VersionId>,
    /// Object size, for events that publish bytes.
    pub size: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire names are the webhook contract; a rename would silently break
    /// every subscriber filtering on them.
    #[test]
    fn event_type_names_are_pinned() {
        for (kind, name) in [
            (StorageEventType::BucketCreated, "bucket.created"),
            (StorageEventType::BucketDeleted, "bucket.deleted"),
            (StorageEventType::ObjectCreated, "object.created"),
            (StorageEventType::ObjectUpdated, "object.updated"),
            (StorageEventType::ObjectDeleted, "object.deleted"),
            (StorageEventType::ObjectRestored, "object.restored"),
            (StorageEventType::MultipartCompleted, "multipart.completed"),
            (StorageEventType::MultipartAborted, "multipart.aborted"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(
                serde_json::to_string(&kind).expect("encode"),
                format!("\"{name}\"")
            );
        }
    }

    #[test]
    fn an_unstated_origin_is_an_ordinary_write() {
        assert_eq!(WriteOrigin::default(), WriteOrigin::Direct);
        // A command written before origins existed decodes as one.
        #[derive(serde::Deserialize)]
        struct Holder {
            #[serde(default)]
            origin: WriteOrigin,
        }
        let holder: Holder = serde_json::from_str("{}").expect("decode");
        assert_eq!(holder.origin, WriteOrigin::Direct);
    }
}
