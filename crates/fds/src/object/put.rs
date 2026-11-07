//! Owned PUT requests and transport.

use super::{insert_header_opt, validate_object};
use super::{ObjectMetadata, UploadBody};
use crate::client::ClientRequest;
use crate::{Client, Result};
use chrono::{DateTime, Utc};
use http::Method;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

/// Single-use PUT request. The body owns bytes or a known-length stream.
#[derive(Debug)]
pub struct PutObjectRequest {
    bucket_name: String,
    object_name: String,
    body: crate::object::UploadBody,
    cache_control: Option<String>,
    content_disposition: Option<String>,
    content_encoding: Option<String>,
    content_type: Option<String>,
    content_md5: Option<String>,
    expect: Option<String>,
    metadata: Option<ObjectMetadata>,
    expires: Option<DateTime<Utc>>,
    timeout: Option<Duration>,
}

crate::de::lenient! {
    /// Result of `put_object` / `complete_multipart_upload`.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct PutObjectResponse {
        pub bucket_name: String,
        pub object_name: String,
        pub access_key_id: String,
        pub signature: String,
        pub expires: i64,
        pub previous_version_id: String,
        pub outside_access: bool,
    }
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
            timeout: None,
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
    /// Set a total deadline for this request only, overriding the client-wide
    /// [`request_timeout`](crate::ClientBuilder::request_timeout) whether that is
    /// set or not.
    ///
    /// The deadline runs from connecting, through uploading the whole body,
    /// until the response has been read; a large upload that is still
    /// progressing fails with a timeout once it passes. By default no total
    /// deadline applies unless the client sets one. The client's connect
    /// timeout and idle [`read_timeout`](crate::ClientBuilder::read_timeout)
    /// still apply; the latter is reset by upload progress, so it only fails an
    /// upload that stops moving. Must be nonzero and representable; otherwise
    /// sending fails with an invalid-input error.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
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
        for (name, value) in [
            ("cache-control", &req.cache_control),
            ("content-disposition", &req.content_disposition),
            ("content-encoding", &req.content_encoding),
            ("content-type", &req.content_type),
            ("content-md5", &req.content_md5),
            ("expect", &req.expect),
        ] {
            insert_header_opt(&mut headers, name, value.as_deref())?;
        }

        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, Method::PUT);
        cr.timeout = req
            .timeout
            .map(|t| crate::config::validate_timeout(t, crate::ErrorKind::InvalidInput))
            .transpose()?;
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
