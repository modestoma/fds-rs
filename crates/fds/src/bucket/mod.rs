//! Bucket operations and the bucket, lifecycle and access-log models.
//!
//! Each operation starts from a [`Client`](crate::Client) method that takes
//! the required arguments and returns an operation builder defined here;
//! optional settings are chained, and `send().await` runs it. See the
//! crate-level "Operation builders" section for the convention.
//!
//! ```no_run
//! # async fn example(client: &fds::Client) -> fds::Result<()> {
//! use fds::acl::BucketCannedAcl;
//! use fds::object::StorageClass;
//!
//! client
//!     .create_bucket("bucket")
//!     .acl([BucketCannedAcl::PublicRead])
//!     .storage_class(StorageClass::new(StorageClass::STANDARD))
//!     .send()
//!     .await?;
//! assert!(client.bucket_exists("bucket").send().await?);
//! # Ok(()) }
//! ```

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::acl::Owner;
use crate::error::{Error, Result};
use crate::object::StorageClass;

/// The shape of a builder whose only required argument is a bucket: private
/// fields, a crate-private constructor and `timeout`. Each operation writes
/// its own `send`.
macro_rules! bucket_builder {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug)]
        #[must_use = "an operation builder does nothing until `send` is awaited"]
        pub struct $name {
            client: crate::Client,
            bucket: String,
            timeout: Option<std::time::Duration>,
        }

        impl $name {
            pub(crate) fn new(client: &crate::Client, bucket: impl Into<String>) -> Self {
                Self {
                    client: client.clone(),
                    bucket: bucket.into(),
                    timeout: None,
                }
            }

            timeout_option!();
        }
    };
}

mod acl;
mod basic;
mod config;

pub use acl::{GetBucketAcl, SetBucketAcl};
pub use basic::{
    BucketExists, CreateBucket, DeleteBucket, GetBucketInfo, ListAuthorizedBuckets, ListBuckets,
    MigrateBucket,
};
pub use config::{
    GetAccessLog, GetLifecycleConfig, SetAccessLog, SetLifecycleConfig, SetLifecycleRule,
};

crate::de::lenient! {
    /// Result of [`Client::get_bucket_info`](crate::Client::get_bucket_info),
    /// and one entry of a [`BucketList`]. Missing or `null` keys read as
    /// empty, zero, `false` or `None`.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct BucketInfo {
        #[serde(rename = "name")]
        pub bucket_name: String,
        /// Creation time, decoded from Unix epoch milliseconds; a negative or
        /// unrepresentable timestamp is a protocol violation.
        pub creation_time: Option<DateTime<Utc>> => "crate::de::opt_millis",
        /// Number of objects, as counted by the service (`numObjects`).
        #[serde(rename = "numObjects")]
        pub object_count: u64,
        /// Bytes used.
        pub used_space: u64,
        pub storage_class: Option<StorageClass>,
        pub allow_outside_access: bool,
        #[serde(rename = "enableSSE")]
        pub enable_sse: bool,
    }
}

crate::de::lenient! {
    /// Result of [`Client::list_buckets`](crate::Client::list_buckets) and
    /// [`Client::list_authorized_buckets`](crate::Client::list_authorized_buckets).
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct BucketList {
        pub owner: Owner,
        pub buckets: Vec<BucketInfo>,
    }
}

/// A lifecycle action item. NOTE: `days` is a float and required.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LifecycleBaseItem {
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

crate::de::lenient! {
    /// A lifecycle rule, the body of
    /// [`Client::set_lifecycle_rule`](crate::Client::set_lifecycle_rule). The
    /// action JSON key is `actions`; actions serialize in [`LifecycleAction`]
    /// (wire string) order. Read the JSON form with `serde_json`.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct LifecycleRule {
        pub id: String,
        pub prefix: String,
        pub enabled: bool,
        #[serde(rename = "actions")]
        pub action: BTreeMap<LifecycleAction, LifecycleBaseItem>,
    }
}

crate::de::lenient! {
    /// Lifecycle configuration: the body of
    /// [`Client::set_lifecycle_config`](crate::Client::set_lifecycle_config)
    /// and the result of
    /// [`Client::get_lifecycle_config`](crate::Client::get_lifecycle_config).
    /// The JSON form is `{"rules":[...]}`; read it with `serde_json`.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct LifecycleConfig {
        pub rules: Vec<LifecycleRule>,
    }
}

crate::de::lenient! {
    /// Access log configuration: the body of
    /// [`Client::set_access_log`](crate::Client::set_access_log) (sent as
    /// given, `bucket_name` included) and the result of
    /// [`Client::get_access_log`](crate::Client::get_access_log).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct AccessLog {
        pub bucket_name: String,
        pub enabled: bool,
        pub log_bucket_name: String,
        pub log_prefix: String,
    }
}

pub(crate) fn validate_bucket(bucket: &str) -> Result<()> {
    if bucket.is_empty() {
        return Err(Error::invalid_input("bucket must be non-empty"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_rule_days_are_float() {
        // Mirrors Test_NewLifecycleRuleFromJSON: days == 164.
        let json =
            br#"{"id":"r","prefix":"p","enabled":true,"actions":{"expiration":{"days":164}}}"#;
        let rule = serde_json::from_slice::<LifecycleRule>(json).unwrap();
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
            let rule = serde_json::from_slice::<LifecycleRule>(json.as_bytes()).unwrap();
            assert_eq!(serde_json::to_string(&rule).unwrap(), *json);
        }
        let config = format!(r#"{{"rules":[{}]}}"#, LIFECYCLE_WIRE.join(","));
        let parsed = serde_json::from_slice::<LifecycleConfig>(config.as_bytes()).unwrap();
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
            let mut rule = serde_json::from_slice::<LifecycleRule>(json.as_bytes()).unwrap();
            rule.action = BTreeMap::from([(action, LifecycleBaseItem { days })]);
            assert_eq!(serde_json::to_string(&rule).unwrap(), json);
        }
        let parsed = serde_json::from_slice::<LifecycleRule>(LIFECYCLE_WIRE[2].as_bytes()).unwrap();
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
