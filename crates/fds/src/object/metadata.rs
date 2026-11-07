//! Object metadata and basic information.

use super::validate_object;
use super::{ObjectMetadata, StorageClass};
use crate::client::ClientRequest;
use crate::metadata::{write_metadata_methods, WriteMetadata};
use crate::{Client, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::Method;
use serde::Deserialize;
use std::time::Duration;

crate::de::lenient! {
    /// Result of `get_object_basic_info`. Missing or `null` keys read as zero,
    /// `None` or empty metadata.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct ObjectBasicInfo {
        pub allow_outside_access: Option<bool>,
        /// Size in bytes; a negative value is a protocol violation.
        pub size: u64,
        /// Upload time, decoded from Unix epoch milliseconds; a negative or
        /// unrepresentable timestamp is a protocol violation.
        pub upload_time: Option<DateTime<Utc>> => "crate::de::opt_millis",
        pub storage_class: Option<StorageClass>,
        /// The metadata the service lists for the object; its
        /// [`entries`](ObjectMetadata::entries) are every listed entry.
        #[serde(rename = "metadataBean")]
        pub metadata: ObjectMetadata => "crate::metadata::entry_list",
    }
}

object_builder! {
    /// Builder for [`Client::get_object_basic_info`].
    GetObjectBasicInfo
}

impl GetObjectBasicInfo {
    /// GET `/{bucket}/{object}?basicInfo`.
    pub async fn send(self) -> Result<ObjectBasicInfo> {
        validate_object(&self.bucket, &self.object)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.query.add("basicInfo", "");
        self.client.do_json(cr).await
    }
}

object_builder! {
    /// Builder for [`Client::get_object_metadata`].
    GetObjectMetadata
}

impl GetObjectMetadata {
    /// GET `/{bucket}/{object}?metadata`; the metadata is parsed from the
    /// response headers. A missing object is an error.
    pub async fn send(self) -> Result<ObjectMetadata> {
        validate_object(&self.bucket, &self.object)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.query.add("metadata", "");
        let resp = self.client.do_request(cr).await?;
        Ok(ObjectMetadata::from_headers(resp.headers()))
    }
}

/// Builder for [`Client::set_object_metadata`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct SetObjectMetadata {
    client: Client,
    bucket: String,
    object: String,
    metadata: WriteMetadata,
    timeout: Option<Duration>,
}

/// The typed values and user metadata form the complete metadata the object
/// gets; anything not set here is not kept.
impl SetObjectMetadata {
    write_metadata_methods!("Sent as given, an empty value included.");

    timeout_option!();

    /// PUT `/{bucket}/{object}?setMetaData` with the metadata as a JSON map
    /// body `{"rawMeta":{...}}`, keys in order. The values are not checked as
    /// header values.
    pub async fn send(self) -> Result<()> {
        validate_object(&self.bucket, &self.object)?;
        let body = self.metadata.to_body()?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.query.add("setMetaData", "");
        cr.body = Some(Bytes::from(body).into());
        self.client.do_discard(cr).await
    }
}

impl Client {
    /// Get an object's basic information: size, upload time, storage class and
    /// metadata.
    pub fn get_object_basic_info(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> GetObjectBasicInfo {
        GetObjectBasicInfo::new(self, bucket, object)
    }

    /// Get an object's metadata with GET `?metadata`.
    ///
    /// [`head_object`](Self::head_object) sends a plain HEAD instead and
    /// reports a missing object as `None`; this request fails for one.
    pub fn get_object_metadata(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> GetObjectMetadata {
        GetObjectMetadata::new(self, bucket, object)
    }

    /// Replace an object's metadata with the typed values and user metadata
    /// set on the builder. It is not a patch: read the current metadata and
    /// carry over what should stay.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// use fds::object::UserMetadata;
    ///
    /// client
    ///     .set_object_metadata("bucket", "key")
    ///     .content_type("application/octet-stream")
    ///     .user_metadata([("owner", "upload-service")].into_iter().collect::<UserMetadata>())
    ///     .send()
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn set_object_metadata(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> SetObjectMetadata {
        SetObjectMetadata {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            metadata: WriteMetadata::default(),
            timeout: None,
        }
    }
}
