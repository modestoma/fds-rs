//! Owned PUT requests and transport.

use super::{insert_header_opt, validate_object};
use crate::client::ClientRequest;
use crate::{Client, HttpMethod, ObjectMetadata, Result, UploadBody};
use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::path::Path;

/// Single-use PUT request. The body owns bytes or a known-length stream.
#[derive(Debug)]
pub struct PutObjectRequest {
    bucket_name: String,
    object_name: String,
    body: crate::UploadBody,
    cache_control: Option<String>,
    content_disposition: Option<String>,
    content_encoding: Option<String>,
    content_type: Option<String>,
    content_md5: Option<String>,
    expect: Option<String>,
    metadata: Option<ObjectMetadata>,
    expires: Option<DateTime<Utc>>,
}

/// Result of `put_object` / `complete_multipart_upload` (Go `PutObjectResponse`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PutObjectResponse {
    #[serde(
        rename = "bucketName",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub bucket_name: String,
    #[serde(
        rename = "objectName",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub object_name: String,
    #[serde(
        rename = "accessKeyId",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub access_key_id: String,
    #[serde(
        rename = "signature",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub signature: String,
    #[serde(
        rename = "expires",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub expires: i64,
    #[serde(
        rename = "previousVersionId",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub previous_version_id: String,
    #[serde(
        rename = "outsideAccess",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub outside_access: bool,
}

impl PutObjectRequest {
    pub fn new(
        bucket: impl Into<String>,
        object: impl Into<String>,
        body: impl Into<UploadBody>,
    ) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_type: None,
            content_md5: None,
            expect: None,
            metadata: None,
            body: body.into(),
            expires: None,
        }
    }
    /// Open a regular file and build a single-use PUT, without sending it.
    /// Keep the file unchanged until completion. Set `content_md5` before sending
    /// if the client requires MD5; neither MIME type nor checksum is inferred.
    pub async fn from_file(
        bucket: impl Into<String>,
        object: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Ok(Self::new(
            bucket,
            object,
            UploadBody::from_file(path).await?,
        ))
    }

    pub fn metadata(mut self, metadata: ObjectMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
    /// Response-signature expiry in UTC, not object TTL.
    pub fn expires(mut self, expires: DateTime<Utc>) -> Self {
        self.expires = Some(expires);
        self
    }
    pub async fn send(self, client: &Client) -> Result<PutObjectResponse> {
        client.put_object(self).await
    }

    pub fn cache_control(mut self, value: impl Into<String>) -> Self {
        self.cache_control = Some(value.into());
        self
    }

    pub fn content_disposition(mut self, value: impl Into<String>) -> Self {
        self.content_disposition = Some(value.into());
        self
    }

    pub fn content_encoding(mut self, value: impl Into<String>) -> Self {
        self.content_encoding = Some(value.into());
        self
    }

    pub fn content_type(mut self, value: impl Into<String>) -> Self {
        self.content_type = Some(value.into());
        self
    }

    pub fn content_md5(mut self, value: impl Into<String>) -> Self {
        self.content_md5 = Some(value.into());
        self
    }

    pub fn expect(mut self, value: impl Into<String>) -> Self {
        self.expect = Some(value.into());
        self
    }
}

impl Client {
    /// Create/overwrite an object, consuming its body without copying or retrying it.
    pub async fn put_object(&self, req: PutObjectRequest) -> Result<PutObjectResponse> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let mut headers = HeaderMap::new();
        insert_header_opt(&mut headers, "cache-control", &req.cache_control)?;
        insert_header_opt(
            &mut headers,
            "content-disposition",
            &req.content_disposition,
        )?;
        insert_header_opt(&mut headers, "content-encoding", &req.content_encoding)?;
        insert_header_opt(&mut headers, "content-type", &req.content_type)?;
        insert_header_opt(&mut headers, "content-md5", &req.content_md5)?;
        insert_header_opt(&mut headers, "expect", &req.expect)?;

        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, HttpMethod::Put);
        cr.headers = headers;
        cr.metadata = req.metadata.as_ref();
        cr.body = Some(req.body);
        if let Some(expires) = req.expires {
            cr.query
                .add("expires", expires.timestamp_millis().to_string());
        }
        self.do_json(cr).await
    }
}
