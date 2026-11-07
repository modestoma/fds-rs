//! Object storage operations, organized by domain.
//! Requests expose constructors/options and delegate execution to Client.
//! Upload requests own their bodies; no compatibility request wrappers exist.

use crate::{Error, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

mod acl;
mod basic;
mod delete;
mod listing;
mod metadata;
mod multipart;
mod presign;
mod put;

pub use acl::{GetObjectACLRequest, SetObjectACLRequest};
pub use basic::{CopyObjectRequest, CopyObjectResponse, GetObjectRequest};
pub use delete::{DeleteObjectFailure, DeleteObjectsOutput};
pub use listing::{ListObjectsRequest, ObjectListing, ObjectSummary};
pub use metadata::{ObjectBasicInfo, SetObjectMetadataRequest};
pub use multipart::{
    CompleteMultipartUploadRequest, InitMultipartUploadRequest, InitMultipartUploadResponse,
    UploadPartRequest, UploadPartResponse,
};
pub use presign::GeneratePresignedURLRequest;
pub use put::{PutObjectRequest, PutObjectResponse};

/// Insert a header only when the option is `Some` and non-empty (Go omitempty).
fn insert_header_opt(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &Option<String>,
) -> Result<()> {
    if let Some(v) = value {
        if !v.is_empty() {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(v).map_err(|e| Error::invalid_input(e.to_string()))?,
            );
        }
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

#[cfg(test)]
mod tests;
