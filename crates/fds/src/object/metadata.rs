//! Object metadata and basic information.

use super::validate_object;
use super::{ObjectMetadata, StorageClass};
use crate::client::ClientRequest;
use crate::{Client, Result};
use bytes::Bytes;
use http::Method;
use serde::Deserialize;

/// Input of `set_object_metadata`.
#[derive(Debug, Clone, Default)]
pub struct SetObjectMetadataRequest {
    pub bucket_name: String,
    pub object_name: String,
    pub metadata: ObjectMetadata,
}

crate::de::lenient! {
    /// Result of `get_object_basic_info`. `metadata` uses the entry-list form
    /// (see [`ObjectMetadata`](super::ObjectMetadata)).
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct ObjectBasicInfo {
        pub allow_outside_access: Option<bool>,
        pub size: i64,
        pub upload_time: i64,
        #[serde(rename = "storageClass")]
        pub storage_class_type: Option<StorageClass>,
        #[serde(rename = "metadataBean")]
        pub metadata: ObjectMetadata,
    }
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
        let mut cr = ClientRequest::new(bucket, object, Method::GET).read_only();
        cr.query.add("basicInfo", "");
        self.do_json(cr).await
    }

    /// Get object metadata (GET `/{b}/{o}?metadata`); parsed from response headers.
    pub async fn get_object_metadata(&self, bucket: &str, object: &str) -> Result<ObjectMetadata> {
        validate_object(bucket, object)?;
        let mut cr = ClientRequest::new(bucket, object, Method::GET).read_only();
        cr.query.add("metadata", "");
        let resp = self.do_request(cr).await?;
        Ok(ObjectMetadata::from_headers(resp.headers()))
    }

    /// Set object metadata (PUT `/{b}/{o}?setMetaData`, body = map form).
    pub async fn set_object_metadata(&self, req: &SetObjectMetadataRequest) -> Result<()> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let body = req.metadata.serialize()?;
        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, Method::PUT);
        cr.query.add("setMetaData", "");
        cr.body = Some(Bytes::from(body).into());
        self.do_discard(cr).await
    }
}
