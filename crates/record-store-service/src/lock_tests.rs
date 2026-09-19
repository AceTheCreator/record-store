//! Object Lock policy at the service layer: bypass auditing and defaults.

use chrono::{Duration, Utc};
use record_store_audit::{AuditQuery, AuditRepository, AuditResult};
use record_store_core::{
    BucketName, ObjectKey, ObjectLockConfiguration, ObjectLockState, Retention, RetentionMode,
};

use crate::test_support::{body, services_with_audit};
use crate::{LockContext, ServiceError, ServicePutRequest};

async fn locked_bucket(services: &crate::Services, name: &str) -> BucketName {
    let bucket = BucketName::new(name).expect("bucket name");
    services
        .buckets
        .create_locked(bucket.clone(), None, true)
        .await
        .expect("create locked bucket");
    bucket
}

/// Every bypass writes a durable record, including one that did not work. An
/// attempted override of a retention is exactly as interesting to an auditor as
/// a successful one.
#[tokio::test]
async fn every_governance_bypass_is_recorded_whether_or_not_it_succeeds() {
    let (_directory, services, audit) = services_with_audit().await;
    let bucket = locked_bucket(&services, "records").await;
    let key = ObjectKey::new("draft.txt").expect("key");

    let governance = services
        .objects
        .put(ServicePutRequest {
            bucket: bucket.clone(),
            key: key.clone(),
            content_type: None,
            custom_metadata: std::collections::BTreeMap::new(),
            expected_checksum: None,
            object_lock: Some(ObjectLockState {
                retention: Some(Retention {
                    mode: RetentionMode::Governance,
                    retain_until: Utc::now() + Duration::days(30),
                }),
                legal_hold: false,
            }),
            body: body(b"draft"),
        })
        .await
        .expect("put under governance retention");

    let compliance = services
        .objects
        .put(ServicePutRequest {
            bucket: bucket.clone(),
            key: ObjectKey::new("final.txt").expect("key"),
            content_type: None,
            custom_metadata: std::collections::BTreeMap::new(),
            expected_checksum: None,
            object_lock: Some(ObjectLockState {
                retention: Some(Retention {
                    mode: RetentionMode::Compliance,
                    retain_until: Utc::now() + Duration::days(30),
                }),
                legal_hold: false,
            }),
            body: body(b"final"),
        })
        .await
        .expect("put under compliance retention");

    let context =
        LockContext::principal("service_account:auditor-test").with_governance_bypass(true);

    // A bypass that works.
    services
        .objects
        .delete_version(
            &bucket,
            key.clone(),
            governance.metadata.version_id,
            &context,
        )
        .await
        .expect("an authorized bypass releases a governance retention");

    // A bypass that does not, because compliance mode has none.
    let refused = services
        .objects
        .delete_version(
            &bucket,
            compliance.metadata.key.clone(),
            compliance.metadata.version_id,
            &context,
        )
        .await
        .expect_err("compliance mode has no bypass");
    assert!(
        matches!(refused, ServiceError::ObjectLocked(_)),
        "{refused}"
    );

    let page = audit
        .query(AuditQuery {
            operation: Some("object-lock.bypass-delete-version".into()),
            limit: 50,
            ..AuditQuery::default()
        })
        .await
        .expect("audit query");
    assert_eq!(page.events.len(), 2, "both attempts are recorded");
    assert!(
        page.events
            .iter()
            .any(|event| event.result == AuditResult::Success)
    );
    assert!(
        page.events
            .iter()
            .any(|event| event.result == AuditResult::Denied)
    );
    for event in &page.events {
        assert_eq!(event.principal, "service_account:auditor-test");
        assert!(
            event.metadata.contains_key("version_id"),
            "a record names the version it concerns"
        );
    }
}

/// An operation that presents no bypass must not produce a bypass record, or
/// the audit trail stops meaning anything.
#[tokio::test]
async fn an_ordinary_delete_writes_no_bypass_record() {
    let (_directory, services, audit) = services_with_audit().await;
    let bucket = locked_bucket(&services, "records").await;
    let key = ObjectKey::new("plain.txt").expect("key");
    let stored = services
        .objects
        .put(ServicePutRequest {
            bucket: bucket.clone(),
            key: key.clone(),
            content_type: None,
            custom_metadata: std::collections::BTreeMap::new(),
            expected_checksum: None,
            object_lock: None,
            body: body(b"plain"),
        })
        .await
        .expect("put");

    services
        .objects
        .delete_version(
            &bucket,
            key,
            stored.metadata.version_id,
            &LockContext::principal("service_account:ordinary"),
        )
        .await
        .expect("an unlocked version deletes normally");

    let page = audit
        .query(AuditQuery {
            operation: Some("object-lock.bypass-delete-version".into()),
            limit: 50,
            ..AuditQuery::default()
        })
        .await
        .expect("audit query");
    assert!(page.events.is_empty(), "no bypass was exercised");
}

#[tokio::test]
async fn a_bucket_default_is_applied_to_writes_that_name_no_lock() {
    let (_directory, services, _audit) = services_with_audit().await;
    let bucket = locked_bucket(&services, "records").await;
    services
        .locks
        .set_bucket_configuration(
            &bucket,
            ObjectLockConfiguration {
                default_retention: Some(record_store_core::DefaultRetention {
                    mode: RetentionMode::Compliance,
                    period: record_store_core::RetentionPeriod::Days(7),
                }),
            },
        )
        .await
        .expect("set default retention");

    let stored = services
        .objects
        .put(ServicePutRequest {
            bucket: bucket.clone(),
            key: ObjectKey::new("auto.txt").expect("key"),
            content_type: None,
            custom_metadata: std::collections::BTreeMap::new(),
            expected_checksum: None,
            object_lock: None,
            body: body(b"auto"),
        })
        .await
        .expect("put");

    let state = services
        .objects
        .version_lock(stored.metadata.version_id)
        .await
        .expect("read lock");
    let retention = state
        .retention
        .expect("the bucket default applies to a write that named no lock");
    assert_eq!(retention.mode, RetentionMode::Compliance);
    assert!(retention.retain_until > Utc::now() + Duration::days(6));
}

/// A background worker is not a person exercising a permission, so it carries
/// no bypass no matter what it is scanning.
#[tokio::test]
async fn a_system_context_never_carries_a_bypass() {
    assert!(!LockContext::system("lifecycle").bypass_governance);
    assert_eq!(
        LockContext::system("lifecycle").principal,
        "system:lifecycle"
    );
}
