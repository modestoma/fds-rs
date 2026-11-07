//! Single-object operations, listing, bulk delete, presigned URLs and the
//! object models. Requests expose constructors/options and delegate execution
//! to [`Client`](crate::Client). Upload requests own their bodies.
//!
//! The low-level multipart requests live in [`crate::multipart`].

use crate::{Error, Result};
use reqwest::header::{HeaderMap, HeaderName};
use serde::{Deserialize, Serialize};

mod acl;
mod basic;
mod delete;
mod listing;
mod metadata;
pub(crate) mod multipart;
mod presign;
mod put;

pub use acl::{GetObjectACLRequest, SetObjectACLRequest};
pub use basic::{CopyObjectRequest, CopyObjectResponse, GetObjectRequest};
pub use delete::{DeleteObjectFailure, DeleteObjectsOutput};
pub use listing::{ListObjectsRequest, ObjectListing, ObjectSummary};
pub use metadata::{ObjectBasicInfo, SetObjectMetadataRequest};
pub use presign::GeneratePresignedURLRequest;
pub use put::{PutObjectRequest, PutObjectResponse};

pub use crate::download::{ByteRange, GetObjectOutput};
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
/// reject those, so we keep a transparent string wrapper (any server value
/// deserializes and round-trips losslessly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StorageClass(pub String);

impl StorageClass {
    /// Canonical value used by `create_bucket` (matches Go's `Standard`).
    pub const STANDARD: &'static str = "STANDARD";
    pub const STANDARD_IA: &'static str = "STANDARD_IA";
    pub const ARCHIVE: &'static str = "Archive";

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
