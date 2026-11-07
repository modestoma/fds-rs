//! Single-object operations, listing, bulk delete, presigned URLs and the
//! object models.
//!
//! Each single-object, listing and bulk-delete operation starts from a
//! [`Client`](crate::Client) method that takes the required arguments and
//! returns an operation builder defined here; optional settings are chained, and `send().await` runs it.
//! See the crate-level "Operation builders" section for the convention.
//!
//! ```no_run
//! # async fn example(client: &fds::Client) -> fds::Result<()> {
//! use std::time::Duration;
//!
//! let head = client
//!     .get_object("bucket", "key")
//!     .range(fds::object::ByteRange::inclusive(0, 1023)?)
//!     .timeout(Duration::from_secs(30))
//!     .send()
//!     .await?
//!     .bytes()
//!     .await?;
//! # let _ = head;
//! # Ok(()) }
//! ```
//!
//! The low-level multipart requests live in [`crate::multipart`].

use crate::{Error, Result};
use reqwest::header::{HeaderMap, HeaderName};
use serde::{Deserialize, Serialize};

/// The shape of a builder whose only required arguments are a bucket and an
/// object: private fields, a crate-private constructor and `timeout`. Each
/// operation writes its own `send`.
macro_rules! object_builder {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug)]
        #[must_use = "an operation builder does nothing until `send` is awaited"]
        pub struct $name {
            client: crate::Client,
            bucket: String,
            object: String,
            timeout: Option<std::time::Duration>,
        }

        impl $name {
            pub(crate) fn new(
                client: &crate::Client,
                bucket: impl Into<String>,
                object: impl Into<String>,
            ) -> Self {
                Self {
                    client: client.clone(),
                    bucket: bucket.into(),
                    object: object.into(),
                    timeout: None,
                }
            }

            timeout_option!();
        }
    };
}

mod acl;
mod basic;
mod delete;
mod listing;
mod metadata;
pub(crate) mod multipart;
mod presign;
mod put;

pub use acl::{GetObjectAcl, SetObjectAcl, SetObjectPublic};
#[cfg(feature = "manager")]
pub(crate) use basic::head_request;
pub use basic::{
    CopyObject, CopyObjectOutput, GetObject, HeadObject, ObjectExists, PrefetchObject,
    RefreshObject, RenameObject, RestoreObject,
};
pub use delete::{
    DeleteObject, DeleteObjectFailure, DeleteObjects, DeleteObjectsOutput, DeletePrefix,
};
pub use listing::{ListObjects, ObjectListing, ObjectSummary};
pub use metadata::{GetObjectBasicInfo, GetObjectMetadata, ObjectBasicInfo, SetObjectMetadata};
pub use presign::PresignObject;
pub use put::{PutObject, PutObjectOutput};

pub use crate::download::{ByteRange, ContentRange, GetObjectOutput};
pub use crate::metadata::ObjectMetadata;
pub use crate::upload::UploadBody;

/// Insert an optional header; `None` and empty values are not sent.
fn insert_header_opt(
    headers: &mut HeaderMap,
    name: &'static str,
    value: Option<&str>,
) -> Result<()> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        headers.insert(
            HeaderName::from_static(name),
            crate::client::header_value(value)?,
        );
    }
    Ok(())
}

pub(crate) fn validate_object(bucket: &str, object: &str) -> Result<()> {
    crate::bucket::validate_bucket(bucket)?;
    if object.is_empty() {
        return Err(Error::invalid_input("bucket and object must be non-empty"));
    }
    Ok(())
}

/// Storage class. FDS models this as `type StorageClass string` (an opaque
/// string), and the value's casing is NOT stable: e.g. `create_bucket` sends
/// `STANDARD`, but `get_bucket_info` may return `Standard`. A strict enum would
/// reject those, so this is a transparent string wrapper: any server value
/// deserializes and round-trips losslessly through [`as_str`](Self::as_str).
///
/// Compare with the case-insensitive [`is_standard`](Self::is_standard),
/// [`is_standard_ia`](Self::is_standard_ia) and
/// [`is_archive`](Self::is_archive); `==` compares the exact strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StorageClass(String);

impl StorageClass {
    /// Canonical value used by `create_bucket` (matches Go's `Standard`).
    pub const STANDARD: &'static str = "STANDARD";
    pub const STANDARD_IA: &'static str = "STANDARD_IA";
    pub const ARCHIVE: &'static str = "Archive";

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value exactly as sent or received.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// [`STANDARD`](Self::STANDARD), ignoring ASCII case.
    pub fn is_standard(&self) -> bool {
        self.0.eq_ignore_ascii_case(Self::STANDARD)
    }

    /// [`STANDARD_IA`](Self::STANDARD_IA), ignoring ASCII case.
    pub fn is_standard_ia(&self) -> bool {
        self.0.eq_ignore_ascii_case(Self::STANDARD_IA)
    }

    /// [`ARCHIVE`](Self::ARCHIVE), ignoring ASCII case.
    pub fn is_archive(&self) -> bool {
        self.0.eq_ignore_ascii_case(Self::ARCHIVE)
    }
}

impl std::fmt::Display for StorageClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for StorageClass {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for StorageClass {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg(test)]
mod tests;
