//! Bucket API (14 methods) + lifecycle / access-log types. Ports `fds/bucket.go`.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::acl::{AccessControlList, Owner};
use crate::client::{Client, ClientRequest};
use crate::encoding::QueryValues;
use crate::error::{Error, Result};
use crate::object::StorageClass;
use http::Method;

// ---- Request/response types ------------------------------------------------

/// Input of `create_bucket` (Go `CreateBucketRequest`).
#[derive(Debug, Clone, Default)]
pub struct CreateBucketRequest {
    pub bucket_name: String,
    /// `storageClass` — omitempty: `None` is not emitted.
    pub storage_class_type: Option<StorageClass>,
    pub acl: Vec<crate::acl::BucketCannedAcl>,
    /// `orgId` — omitempty: empty is not emitted.
    pub org_id: String,
}

impl CreateBucketRequest {
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket_name: bucket.into(),
            ..Self::default()
        }
    }
    pub fn storage_class(mut self, value: StorageClass) -> Self {
        self.storage_class_type = Some(value);
        self
    }
    pub fn org_id(mut self, value: impl Into<String>) -> Self {
        self.org_id = value.into();
        self
    }
    /// Empty grants preserve the service default.
    pub fn acl(mut self, value: impl IntoIterator<Item = crate::acl::BucketCannedAcl>) -> Self {
        self.acl = value.into_iter().collect();
        self
    }
    pub async fn send(self, client: &Client) -> Result<()> {
        client.create_bucket(&self).await
    }
}

/// Add (default) or delete exactly the supplied bucket grants.
#[derive(Debug, Clone)]
pub struct SetBucketAclRequest {
    pub bucket_name: String,
    pub acl: AccessControlList,
    pub action: crate::acl::AclAction,
}
impl SetBucketAclRequest {
    pub fn new(bucket: impl Into<String>, acl: AccessControlList) -> Self {
        Self {
            bucket_name: bucket.into(),
            acl,
            action: crate::acl::AclAction::Add,
        }
    }
    pub fn action(mut self, action: crate::acl::AclAction) -> Self {
        self.action = action;
        self
    }
    pub async fn send(self, client: &Client) -> Result<()> {
        client.set_bucket_acl(&self).await
    }
}

/// Result of `get_bucket_info` (Go `GetBucketInfoResponse`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GetBucketInfoResponse {
    #[serde(
        rename = "allowOutsideAccess",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub allow_outside_access: bool,
    #[serde(
        rename = "enableSSE",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub enable_sse: bool,
    #[serde(
        rename = "creationTime",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub creation_time: i64,
    #[serde(rename = "name", default, deserialize_with = "crate::de::null_default")]
    pub bucket_name: String,
    #[serde(
        rename = "numObjects",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub object_num: i64,
    #[serde(
        rename = "usedSpace",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub used_space: i64,
    #[serde(
        rename = "storageClass",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub storage_class_type: Option<StorageClass>,
}

/// Result of `list_buckets` / `list_authorized_buckets` (Go `ListBucketsResponse`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListBucketsResponse {
    #[serde(
        rename = "owner",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub owner: Owner,
    #[serde(
        rename = "buckets",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub buckets: Vec<GetBucketInfoResponse>,
}

/// Input of `migrate_bucket` (Go `MigrateBucketRequest`). `orgId`/`teamId` have
/// no omitempty, so they are always emitted.
#[derive(Debug, Clone, Default)]
pub struct MigrateBucketRequest {
    pub bucket_name: String,
    pub org_id: String,
    pub team_id: String,
}

/// A lifecycle action item (Go `LifecycleBaseItem`). NOTE: `days` is a float.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LifecycleBaseItem {
    #[serde(rename = "days")]
    pub days: f64,
}

/// A lifecycle action type: the key of [`LifecycleRule::action`].
///
/// Known values serialize to the exact strings the service expects, including
/// the trailing space the Go SDK sends in `"abortIncompleteMultipartUpload "`.
/// Any other key, including `"abortIncompleteMultipartUpload"` without the
/// space, round-trips byte-for-byte through [`LifecycleAction::Other`].
///
/// Equality, ordering and hashing compare the wire string, so the actions of
/// a rule serialize in sorted key order, as Go's `encoding/json` does.
///
/// ```
/// use fds::bucket::LifecycleAction;
///
/// assert_eq!(LifecycleAction::AbortIncompleteMultipartUpload.as_str(), "abortIncompleteMultipartUpload ");
/// assert_eq!(LifecycleAction::from("expiration"), LifecycleAction::Expiration);
/// assert_eq!(LifecycleAction::from("transition").as_str(), "transition");
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum LifecycleAction {
    /// `"abortIncompleteMultipartUpload "`. The trailing space is the Go SDK's
    /// wire value and is kept for server compatibility.
    AbortIncompleteMultipartUpload,
    /// `"expiration"`.
    Expiration,
    /// `"nonCurrentVersionExpiration"`.
    NonCurrentVersionExpiration,
    /// Any other action key, kept verbatim.
    Other(String),
}

impl LifecycleAction {
    const ABORT_INCOMPLETE_MULTIPART_UPLOAD: &'static str = "abortIncompleteMultipartUpload ";
    const EXPIRATION: &'static str = "expiration";
    const NON_CURRENT_VERSION_EXPIRATION: &'static str = "nonCurrentVersionExpiration";

    /// The exact JSON key sent to and received from the service.
    pub fn as_str(&self) -> &str {
        match self {
            Self::AbortIncompleteMultipartUpload => Self::ABORT_INCOMPLETE_MULTIPART_UPLOAD,
            Self::Expiration => Self::EXPIRATION,
            Self::NonCurrentVersionExpiration => Self::NON_CURRENT_VERSION_EXPIRATION,
            Self::Other(value) => value,
        }
    }
}

impl From<&str> for LifecycleAction {
    fn from(value: &str) -> Self {
        match value {
            Self::ABORT_INCOMPLETE_MULTIPART_UPLOAD => Self::AbortIncompleteMultipartUpload,
            Self::EXPIRATION => Self::Expiration,
            Self::NON_CURRENT_VERSION_EXPIRATION => Self::NonCurrentVersionExpiration,
            other => Self::Other(other.to_owned()),
        }
    }
}

impl From<String> for LifecycleAction {
    fn from(value: String) -> Self {
        match Self::from(value.as_str()) {
            Self::Other(_) => Self::Other(value),
            known => known,
        }
    }
}

impl PartialEq for LifecycleAction {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for LifecycleAction {}

impl std::hash::Hash for LifecycleAction {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl PartialOrd for LifecycleAction {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LifecycleAction {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Serialize for LifecycleAction {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LifecycleAction {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

/// A lifecycle rule (Go `LifecycleRule`). The action JSON key is `actions`;
/// actions serialize in [`LifecycleAction`] (wire string) order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LifecycleRule {
    #[serde(rename = "id", default, deserialize_with = "crate::de::null_default")]
    pub id: String,
    #[serde(
        rename = "prefix",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub prefix: String,
    #[serde(
        rename = "enabled",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub enabled: bool,
    #[serde(
        rename = "actions",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub action: BTreeMap<LifecycleAction, LifecycleBaseItem>,
}

impl LifecycleRule {
    pub fn from_json(content: &[u8]) -> Result<Self> {
        serde_json::from_slice(content).map_err(Error::json_input)
    }
}

/// Lifecycle configuration (Go `LifecycleConfig`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LifecycleConfig {
    #[serde(
        rename = "rules",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub rules: Vec<LifecycleRule>,
}

impl LifecycleConfig {
    pub fn from_json(content: &[u8]) -> Result<Self> {
        serde_json::from_slice(content).map_err(Error::json_input)
    }
}

/// Input of `get_lifecycle_config` (Go `GetLifecycleConfigRequest`).
#[derive(Debug, Clone, Default)]
pub struct GetLifecycleConfigRequest {
    pub bucket_name: String,
    /// `lifecycle=<rule_id>` (no omitempty).
    pub rule_id: String,
}

/// Access log configuration (Go `AccessLog`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccessLog {
    #[serde(
        rename = "bucketName",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub bucket_name: String,
    #[serde(
        rename = "enabled",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub enabled: bool,
    #[serde(
        rename = "logBucketName",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub log_bucket_name: String,
    #[serde(
        rename = "logPrefix",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub log_prefix: String,
}

impl MigrateBucketRequest {
    pub fn new(
        bucket: impl Into<String>,
        org_id: impl Into<String>,
        team_id: impl Into<String>,
    ) -> Self {
        Self {
            bucket_name: bucket.into(),
            org_id: org_id.into(),
            team_id: team_id.into(),
        }
    }
    pub async fn send(self, client: &Client) -> Result<()> {
        client.migrate_bucket(&self).await
    }
}
impl GetLifecycleConfigRequest {
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket_name: bucket.into(),
            ..Self::default()
        }
    }
    pub fn rule_id(mut self, value: impl Into<String>) -> Self {
        self.rule_id = value.into();
        self
    }
    pub async fn send(self, client: &Client) -> Result<LifecycleConfig> {
        client.get_lifecycle_config(&self).await
    }
}

pub(crate) fn validate_bucket(bucket: &str) -> Result<()> {
    if bucket.is_empty() {
        return Err(Error::invalid_input("bucket must be non-empty"));
    }
    Ok(())
}

// ---- Methods ---------------------------------------------------------------

impl Client {
    /// Create a bucket (PUT `/{b}?storageClass?orgId`, empty body).
    pub async fn create_bucket(&self, req: &CreateBucketRequest) -> Result<()> {
        validate_bucket(&req.bucket_name)?;
        let acl = &req.acl;
        let mut q = QueryValues::new();
        if let Some(sc) = &req.storage_class_type {
            q.add("storageClass", sc.as_str());
        }
        if !req.org_id.is_empty() {
            q.add("orgId", &req.org_id);
        }
        let mut cr = ClientRequest::new(&req.bucket_name, "", Method::PUT);
        cr.query = q;
        if !acl.is_empty() {
            let value = acl.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(",");
            cr.headers.insert(
                "x-xiaomi-meta-acl",
                value
                    .parse()
                    .map_err(|e: reqwest::header::InvalidHeaderValue| {
                        Error::invalid_input(e.to_string())
                    })?,
            );
        }
        cr.body = Some(Bytes::new().into()); // empty PUT body (Content-Length: 0)
        self.do_discard(cr).await
    }

    /// Whether a bucket exists (HEAD; 200 → true, 404 → false).
    pub async fn does_bucket_exist(&self, bucket: &str) -> Result<bool> {
        validate_bucket(bucket)?;
        let resp = self
            .do_request({
                let mut cr = ClientRequest::new(bucket, "", Method::HEAD).read_only();
                cr.accepted_statuses = &[200, 404];
                cr
            })
            .await?;
        Ok(resp.status().as_u16() == 200)
    }

    /// Delete a bucket (DELETE `/{b}`).
    pub async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        validate_bucket(bucket)?;
        self.do_discard(ClientRequest::new(bucket, "", Method::DELETE))
            .await
    }

    /// Get bucket info (GET `/{b}`).
    pub async fn get_bucket_info(&self, bucket: &str) -> Result<GetBucketInfoResponse> {
        validate_bucket(bucket)?;
        self.do_json(ClientRequest::new(bucket, "", Method::GET).read_only())
            .await
    }

    /// List all buckets (GET `/`).
    pub async fn list_buckets(&self) -> Result<ListBucketsResponse> {
        self.do_json(ClientRequest::new("", "", Method::GET).read_only())
            .await
    }

    /// List buckets the user can access (GET `/?authorizedBuckets`).
    pub async fn list_authorized_buckets(&self) -> Result<ListBucketsResponse> {
        let mut cr = ClientRequest::new("", "", Method::GET).read_only();
        cr.query.add("authorizedBuckets", "");
        self.do_json(cr).await
    }

    /// Migrate a bucket to another org/team (PUT `/{b}?migrate&orgId&teamId`).
    pub async fn migrate_bucket(&self, req: &MigrateBucketRequest) -> Result<()> {
        validate_bucket(&req.bucket_name)?;
        let mut q = QueryValues::new();
        q.add("migrate", "");
        q.add("orgId", &req.org_id);
        q.add("teamId", &req.team_id);
        let mut cr = ClientRequest::new(&req.bucket_name, "", Method::PUT);
        cr.query = q;
        cr.body = Some(Bytes::new().into());
        self.do_discard(cr).await
    }

    /// Get bucket ACL (GET `/{b}?acl`).
    pub async fn get_bucket_acl(&self, bucket: &str) -> Result<AccessControlList> {
        validate_bucket(bucket)?;
        let mut cr = ClientRequest::new(bucket, "", Method::GET).read_only();
        cr.query.add("acl", "");
        self.do_json(cr).await
    }

    /// Set bucket ACL (PUT `/{b}?acl`, body = JSON ACL).
    pub async fn set_bucket_acl(&self, req: &SetBucketAclRequest) -> Result<()> {
        validate_bucket(&req.bucket_name)?;
        let bucket = &req.bucket_name;
        let acl = &req.acl;
        let action = req.action;
        let body = serde_json::to_vec(acl).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(bucket, "", Method::PUT);
        cr.query.add("acl", "");
        action.apply(&mut cr.query);
        cr.body = Some(Bytes::from(body).into());
        self.do_discard(cr).await
    }

    /// Get lifecycle config (GET `/{b}?lifecycle=<rule_id>`).
    pub async fn get_lifecycle_config(
        &self,
        req: &GetLifecycleConfigRequest,
    ) -> Result<LifecycleConfig> {
        validate_bucket(&req.bucket_name)?;
        let mut cr = ClientRequest::new(&req.bucket_name, "", Method::GET).read_only();
        cr.query.add("lifecycle", &req.rule_id);
        self.do_json(cr).await
    }

    /// Set lifecycle config (PUT `/{b}?lifecycle`, body = JSON config).
    pub async fn set_lifecycle_config(&self, bucket: &str, config: &LifecycleConfig) -> Result<()> {
        validate_bucket(bucket)?;
        let body = serde_json::to_vec(config).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(bucket, "", Method::PUT);
        cr.query.add("lifecycle", "");
        cr.body = Some(Bytes::from(body).into());
        self.do_discard(cr).await
    }

    /// Set a single lifecycle rule (PUT `/{b}?lifecycle=rule`, body = JSON rule).
    pub async fn set_lifecycle_rule(&self, bucket: &str, rule: &LifecycleRule) -> Result<()> {
        validate_bucket(bucket)?;
        let body = serde_json::to_vec(rule).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(bucket, "", Method::PUT);
        cr.query.add("lifecycle", "rule");
        cr.body = Some(Bytes::from(body).into());
        self.do_discard(cr).await
    }

    /// Get access log (GET `/{b}?accessLog`).
    pub async fn get_access_log(&self, bucket: &str) -> Result<AccessLog> {
        validate_bucket(bucket)?;
        let mut cr = ClientRequest::new(bucket, "", Method::GET).read_only();
        cr.query.add("accessLog", "");
        self.do_json(cr).await
    }

    /// Set access log (PUT `/{b}?accessLog`, body = JSON access log).
    pub async fn set_access_log(&self, bucket: &str, access_log: &AccessLog) -> Result<()> {
        validate_bucket(bucket)?;
        let body = serde_json::to_vec(access_log).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(bucket, "", Method::PUT);
        cr.query.add("accessLog", "");
        cr.body = Some(Bytes::from(body).into());
        self.do_discard(cr).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_rule_from_json_days_float() {
        // Mirrors Test_NewLifecycleRuleFromJSON: days == 164.
        let json =
            br#"{"id":"r","prefix":"p","enabled":true,"actions":{"expiration":{"days":164}}}"#;
        let rule = LifecycleRule::from_json(json).unwrap();
        assert_eq!(rule.action[&LifecycleAction::Expiration].days, 164.0);
    }

    /// Wire bytes captured from the string-keyed map representation that
    /// preceded the `LifecycleAction` enum. One action per rule: the old
    /// `HashMap` had no stable order for several keys.
    const LIFECYCLE_WIRE: &[&str] = &[
        r#"{"id":"r1","prefix":"logs/","enabled":true,"actions":{"expiration":{"days":30.0}}}"#,
        r#"{"id":"r2","prefix":"","enabled":false,"actions":{"nonCurrentVersionExpiration":{"days":1.5}}}"#,
        r#"{"id":"r3","prefix":"tmp/","enabled":true,"actions":{"abortIncompleteMultipartUpload ":{"days":7.0}}}"#,
        r#"{"id":"r4","prefix":"x","enabled":true,"actions":{"abortIncompleteMultipartUpload":{"days":2.0}}}"#,
        r#"{"id":"r5","prefix":"y","enabled":true,"actions":{"transition":{"days":0.0}}}"#,
        r#"{"id":"r6","prefix":"z","enabled":true,"actions":{}}"#,
    ];

    #[test]
    fn lifecycle_wire_bytes_round_trip() {
        for json in LIFECYCLE_WIRE {
            let rule = LifecycleRule::from_json(json.as_bytes()).unwrap();
            assert_eq!(serde_json::to_string(&rule).unwrap(), *json);
        }
        let config = format!(r#"{{"rules":[{}]}}"#, LIFECYCLE_WIRE.join(","));
        let parsed = LifecycleConfig::from_json(config.as_bytes()).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), config);
    }

    #[test]
    fn abort_action_keeps_trailing_space() {
        assert_eq!(
            LifecycleAction::AbortIncompleteMultipartUpload.as_str(),
            "abortIncompleteMultipartUpload "
        );
        // Without the space it is a different, verbatim key.
        let trimmed = LifecycleAction::from("abortIncompleteMultipartUpload");
        assert_eq!(
            trimmed,
            LifecycleAction::Other("abortIncompleteMultipartUpload".into())
        );
        assert_ne!(trimmed, LifecycleAction::AbortIncompleteMultipartUpload);
    }

    #[test]
    fn lifecycle_typed_actions_match_wire_bytes() {
        // Built through the enum: same bytes as the captured map encoding.
        let typed = [
            (LifecycleAction::Expiration, 30.0, LIFECYCLE_WIRE[0]),
            (
                LifecycleAction::NonCurrentVersionExpiration,
                1.5,
                LIFECYCLE_WIRE[1],
            ),
            (
                LifecycleAction::AbortIncompleteMultipartUpload,
                7.0,
                LIFECYCLE_WIRE[2],
            ),
        ];
        for (action, days, json) in typed {
            let mut rule = LifecycleRule::from_json(json.as_bytes()).unwrap();
            rule.action = BTreeMap::from([(action, LifecycleBaseItem { days })]);
            assert_eq!(serde_json::to_string(&rule).unwrap(), json);
        }
        let parsed = LifecycleRule::from_json(LIFECYCLE_WIRE[2].as_bytes()).unwrap();
        assert!(parsed
            .action
            .contains_key(&LifecycleAction::AbortIncompleteMultipartUpload));
    }

    #[test]
    fn lifecycle_multiple_actions_use_sorted_go_key_order() {
        let rule = LifecycleRule {
            id: "r".into(),
            prefix: String::new(),
            enabled: true,
            action: BTreeMap::from([
                (
                    LifecycleAction::NonCurrentVersionExpiration,
                    LifecycleBaseItem { days: 2.0 },
                ),
                (
                    LifecycleAction::Other("transition".into()),
                    LifecycleBaseItem { days: 3.0 },
                ),
                (LifecycleAction::Expiration, LifecycleBaseItem { days: 1.0 }),
                (
                    LifecycleAction::AbortIncompleteMultipartUpload,
                    LifecycleBaseItem { days: 4.0 },
                ),
            ]),
        };
        assert_eq!(
            serde_json::to_string(&rule).unwrap(),
            r#"{"id":"r","prefix":"","enabled":true,"actions":{"abortIncompleteMultipartUpload ":{"days":4.0},"expiration":{"days":1.0},"nonCurrentVersionExpiration":{"days":2.0},"transition":{"days":3.0}}}"#
        );
    }
}
