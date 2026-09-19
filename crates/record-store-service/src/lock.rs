//! Object Lock policy: who may release a retained version, and on what clock.
//!
//! The catalog enforces the invariant — it refuses, inside the transaction, to
//! remove a version a retention still holds. This module decides the policy
//! around that: it resolves what lock a new version is born with, applies the
//! S3 mode rules to a requested change, and records every governance bypass in
//! the durable audit trail.

use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};

use chrono::Utc;
use record_store_audit::{AuditEvent, AuditRepository, AuditResult};
use record_store_core::{
    AuditEventId, Bucket, BucketName, ObjectKey, ObjectLockConfiguration, ObjectLockState,
    VersionId,
};
use record_store_metadata::{LockRelease, MetadataRepository};
use tokio::sync::Semaphore;
use tracing::warn;

use crate::error::map_metadata;
use crate::services::BucketCoordinator;
use crate::*;

/// Who is asking, and whether they carry an authorized governance bypass.
///
/// The bypass flag means the permission was already checked. Presenting
/// `x-amz-bypass-governance-retention` requires `s3:BypassGovernanceRetention`,
/// so a caller without that permission is refused before reaching this layer
/// and can never set this to true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockContext {
    /// Stable non-secret principal name, as it appears in audit records.
    pub principal: String,
    /// Whether an authorized governance bypass was presented.
    pub bypass_governance: bool,
}

impl LockContext {
    /// Builds a context for an ordinary caller presenting no bypass.
    #[must_use]
    pub fn principal(principal: impl Into<String>) -> Self {
        Self {
            principal: principal.into(),
            bypass_governance: false,
        }
    }

    /// Builds a context for an internal component, which never bypasses.
    ///
    /// A background worker is not a person exercising a permission, so it gets
    /// no bypass regardless of what it is scanning.
    #[must_use]
    pub fn system(component: &str) -> Self {
        Self::principal(format!("system:{component}"))
    }

    /// Returns the same context with an authorized bypass applied.
    #[must_use]
    pub const fn with_governance_bypass(mut self, bypass: bool) -> Self {
        self.bypass_governance = bypass;
        self
    }
}

/// Deployment-wide Object Lock settings shared by the services that need them.
pub(crate) struct LockPolicy {
    pub(crate) clock_tolerance_seconds: u32,
    pub(crate) audit: Option<Arc<dyn AuditRepository>>,
}

impl LockPolicy {
    /// Builds the clock and bypass inputs for one operation.
    pub(crate) fn release(&self, context: &LockContext) -> LockRelease {
        LockRelease::new(Utc::now(), self.clock_tolerance_seconds)
            .with_governance_bypass(context.bypass_governance)
    }

    /// Records an exercised governance bypass.
    ///
    /// A bypass is the one way a retained version leaves before its date, so it
    /// is written whether or not the operation that used it went on to succeed.
    /// The record names the version by its identifier and carries no credential.
    pub(crate) async fn record_bypass(
        &self,
        context: &LockContext,
        operation: &str,
        bucket: &BucketName,
        key: &ObjectKey,
        version_id: VersionId,
        result: AuditResult,
    ) {
        let Some(audit) = &self.audit else {
            // Without a durable audit trail a bypass would leave no trace, which
            // is worse than noisy. It is surfaced rather than dropped quietly.
            warn!(
                operation,
                principal = %context.principal,
                "governance bypass exercised with no durable audit trail configured"
            );
            return;
        };
        let mut metadata = BTreeMap::new();
        metadata.insert("version_id".into(), version_id.to_string());
        metadata.insert("bypass".into(), "governance".into());
        let event = AuditEvent {
            event_id: AuditEventId::new(),
            timestamp: Utc::now(),
            request_id: None,
            principal: context.principal.clone(),
            credential_id: None,
            source_ip: None,
            operation: operation.to_owned(),
            resource: format!("bucket:{bucket}/{key}"),
            result,
            metadata,
        };
        if let Err(error) = audit.append(&event).await {
            warn!(%error, operation, "durable object lock bypass audit append failed");
        }
    }
}

/// The Object Lock state of one named version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionLock {
    /// Version the state belongs to.
    pub version_id: VersionId,
    /// Retention and legal hold currently in force.
    pub state: ObjectLockState,
}

/// Object Lock service shared by every protocol.
pub struct ObjectLockService {
    pub(crate) metadata: Arc<dyn MetadataRepository>,
    pub(crate) coordinator: Arc<BucketCoordinator>,
    pub(crate) operations: Arc<Semaphore>,
    pub(crate) metrics: Arc<ServiceMetrics>,
    pub(crate) policy: Arc<LockPolicy>,
}

impl ObjectLockService {
    /// Returns a bucket's Object Lock configuration.
    ///
    /// A bucket without Object Lock is reported as a missing configuration
    /// rather than an empty one, because "never enabled" and "enabled with no
    /// default rule" are different answers and clients branch on them.
    pub async fn bucket_configuration(
        &self,
        bucket_name: &BucketName,
    ) -> Result<ObjectLockConfiguration, ServiceError> {
        self.metrics.requests.fetch_add(1, Ordering::Relaxed);
        let _permit = self.acquire().await?;
        self.resolve_bucket(bucket_name)
            .await?
            .object_lock
            .ok_or(ServiceError::ObjectLockConfigurationNotFound)
    }

    /// Replaces a bucket's default retention.
    ///
    /// Object Lock itself cannot be turned on here; that only happens when the
    /// bucket is created.
    pub async fn set_bucket_configuration(
        &self,
        bucket_name: &BucketName,
        configuration: ObjectLockConfiguration,
    ) -> Result<Bucket, ServiceError> {
        configuration.validate()?;
        self.metrics.requests.fetch_add(1, Ordering::Relaxed);
        let _permit = self.acquire().await?;
        let bucket = self.resolve_bucket(bucket_name).await?;
        let lock = self.coordinator.lock(bucket.id)?;
        let _guard = lock.write().await;
        self.metadata
            .set_bucket_object_lock(bucket.id, configuration)
            .await
            .map_err(map_metadata)
    }

    /// Returns the Object Lock state of one version, current by default.
    pub async fn get(
        &self,
        bucket_name: &BucketName,
        key: &ObjectKey,
        version_id: Option<VersionId>,
    ) -> Result<VersionLock, ServiceError> {
        self.metrics.requests.fetch_add(1, Ordering::Relaxed);
        let _permit = self.acquire().await?;
        let bucket = self.resolve_bucket(bucket_name).await?;
        if bucket.object_lock.is_none() {
            return Err(ServiceError::ObjectLockNotEnabled);
        }
        let version_id = self.resolve_version(&bucket, key, version_id).await?;
        let state = self
            .metadata
            .get_object_lock(version_id)
            .await
            .map_err(map_metadata)?;
        Ok(VersionLock { version_id, state })
    }

    /// Returns the Object Lock state of a version already resolved elsewhere.
    ///
    /// Used on the read path, where the version is known and only the response
    /// headers are still missing.
    pub async fn state_of(&self, version_id: VersionId) -> Result<ObjectLockState, ServiceError> {
        self.metadata
            .get_object_lock(version_id)
            .await
            .map_err(map_metadata)
    }

    /// Replaces the retention of one version.
    ///
    /// Extending is always allowed. Shortening or removing is refused outright
    /// in compliance mode, and in governance mode requires an authorized
    /// bypass, which is audited whether it succeeds or not.
    pub async fn put_retention(
        &self,
        bucket_name: &BucketName,
        key: &ObjectKey,
        version_id: Option<VersionId>,
        retention: Option<record_store_core::Retention>,
        context: &LockContext,
    ) -> Result<VersionLock, ServiceError> {
        self.change(
            bucket_name,
            key,
            version_id,
            context,
            "object-lock.put-retention",
            move |current| ObjectLockState {
                retention,
                ..current
            },
        )
        .await
    }

    /// Places or removes the legal hold on one version.
    ///
    /// A hold is independent of retention: it blocks deletion on its own, in
    /// either mode, and no bypass applies to it. Removing one is a release, so
    /// it is judged against the observed-time high-water mark.
    pub async fn put_legal_hold(
        &self,
        bucket_name: &BucketName,
        key: &ObjectKey,
        version_id: Option<VersionId>,
        legal_hold: bool,
        context: &LockContext,
    ) -> Result<VersionLock, ServiceError> {
        self.change(
            bucket_name,
            key,
            version_id,
            context,
            "object-lock.put-legal-hold",
            move |current| current.with_legal_hold(legal_hold),
        )
        .await
    }

    /// Resolves the Object Lock state a version written now is born with.
    ///
    /// An explicit request wins over the bucket default, and a bucket without
    /// Object Lock refuses an explicit request rather than dropping it.
    pub(crate) fn initial_state(
        bucket: &Bucket,
        requested: Option<ObjectLockState>,
    ) -> Result<Option<ObjectLockState>, ServiceError> {
        match (bucket.object_lock, requested) {
            (None, Some(state)) if !state.is_unlocked() => Err(ServiceError::ObjectLockNotEnabled),
            (None, _) => Ok(None),
            (Some(_), Some(state)) => Ok(Some(state)),
            (Some(configuration), None) => Ok(Some(configuration.initial_state_at(Utc::now())?)),
        }
    }

    /// Advances the observed-time high-water mark retention is judged against.
    ///
    /// Called on a timer as well as by lock operations, so that a deployment
    /// that does nothing lock-related for a month still notices a clock that
    /// went backwards while it was idle.
    pub async fn observe_clock(&self) -> Result<(), ServiceError> {
        let release = self.policy.release(&LockContext::system("clock"));
        match self.metadata.observe_clock(release).await {
            Ok(()) => Ok(()),
            Err(record_store_metadata::MetadataError::ClockWentBackwards) => {
                warn!(
                    "system clock is behind the recorded high-water mark; \
                     object lock will refuse to release retained versions until it catches up"
                );
                Err(ServiceError::RetentionClockUnavailable)
            }
            Err(error) => Err(map_metadata(error)),
        }
    }

    async fn change<F>(
        &self,
        bucket_name: &BucketName,
        key: &ObjectKey,
        version_id: Option<VersionId>,
        context: &LockContext,
        operation: &str,
        apply: F,
    ) -> Result<VersionLock, ServiceError>
    where
        F: FnOnce(ObjectLockState) -> ObjectLockState + Send,
    {
        self.metrics.requests.fetch_add(1, Ordering::Relaxed);
        let _permit = self.acquire().await?;
        let bucket = self.resolve_bucket(bucket_name).await?;
        if bucket.object_lock.is_none() {
            return Err(ServiceError::ObjectLockNotEnabled);
        }
        let version_id = self.resolve_version(&bucket, key, version_id).await?;
        let lock = self.coordinator.lock(bucket.id)?;
        let _guard = lock.read().await;
        let current = self
            .metadata
            .get_object_lock(version_id)
            .await
            .map_err(map_metadata)?;
        let requested = apply(current);
        let release = self.policy.release(context);
        let result = self
            .metadata
            .put_object_lock(bucket.id, key, version_id, requested, release)
            .await;
        if context.bypass_governance {
            let outcome = if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Denied
            };
            self.policy
                .record_bypass(context, operation, bucket_name, key, version_id, outcome)
                .await;
        }
        let state = result.map_err(map_metadata)?;
        Ok(VersionLock { version_id, state })
    }

    async fn resolve_version(
        &self,
        bucket: &Bucket,
        key: &ObjectKey,
        version_id: Option<VersionId>,
    ) -> Result<VersionId, ServiceError> {
        if let Some(version_id) = version_id {
            return Ok(version_id);
        }
        match self
            .metadata
            .get_object(bucket.id, key)
            .await
            .map_err(map_metadata)?
        {
            Some(metadata) => Ok(metadata.version_id),
            None => Err(ServiceError::ObjectNotFound),
        }
    }

    async fn resolve_bucket(&self, name: &BucketName) -> Result<Bucket, ServiceError> {
        self.metadata
            .get_bucket_by_name(name)
            .await
            .map_err(ServiceError::Metadata)?
            .ok_or(ServiceError::BucketNotFound)
    }

    async fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit, ServiceError> {
        Arc::clone(&self.operations)
            .acquire_owned()
            .await
            .map_err(|_| ServiceError::Unavailable)
    }
}
