//! Object metadata and basic information.

use super::validate_object;
use crate::client::ClientRequest;
use crate::{Client, HttpMethod, ObjectMetadata, Result, StorageClass};
use bytes::Bytes;
use serde::Deserialize;

/// Input of `set_object_metadata` (Go `SetObjectMetadataRequest`).
#[derive(Debug, Clone, Default)]
pub struct SetObjectMetadataRequest {
    pub bucket_name: String,
    pub object_name: String,
    pub metadata: ObjectMetadata,
}

/// Result of `get_object_basic_info` (Go `ObjectBasicInfo`). `metadata` uses the
/// entry-list form (see [`crate::metadata`]).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObjectBasicInfo {
    #[serde(
        rename = "allowOutsideAccess",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub allow_outside_access: Option<bool>,
    #[serde(rename = "size", default, deserialize_with = "crate::de::null_default")]
    pub size: i64,
    #[serde(
        rename = "uploadTime",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub upload_time: i64,
    #[serde(
        rename = "storageClass",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub storage_class_type: Option<StorageClass>,
    #[serde(
        rename = "metadataBean",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub metadata: ObjectMetadata,
}

impl SetObjectMetadataRequest {
    pub fn new(
        bucket: impl Into<String>,
        object: impl Into<String>,
        metadata: ObjectMetadata,
    ) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            metadata,
        }
    }
    pub async fn send(self, client: &Client) -> Result<()> {
        client.set_object_metadata(&self).await
    }
}

impl Client {
    /// Get an object's basic info (GET `/{b}/{o}?basicInfo`).
    pub async fn get_object_basic_info(
        &self,
        bucket: &str,
        object: &str,
    ) -> Result<ObjectBasicInfo> {
        validate_object(bucket, object)?;
        let mut cr = ClientRequest::new(bucket, object, HttpMethod::Get).read_only();
        cr.query.add("basicInfo", "");
        self.do_json(cr).await
    }

    /// Get object metadata (GET `/{b}/{o}?metadata`); parsed from response headers.
    pub async fn get_object_metadata(&self, bucket: &str, object: &str) -> Result<ObjectMetadata> {
        validate_object(bucket, object)?;
        let mut cr = ClientRequest::new(bucket, object, HttpMethod::Get).read_only();
        cr.query.add("metadata", "");
        let resp = self.do_request(cr).await?;
        Ok(ObjectMetadata::from_headers(resp.headers()))
    }

    /// Set object metadata (PUT `/{b}/{o}?setMetaData`, body = map form).
    pub async fn set_object_metadata(&self, req: &SetObjectMetadataRequest) -> Result<()> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let body = req.metadata.serialize()?;
        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, HttpMethod::Put);
        cr.query.add("setMetaData", "");
        cr.body = Some(Bytes::from(body).into());
        self.do_discard(cr).await
    }
}
