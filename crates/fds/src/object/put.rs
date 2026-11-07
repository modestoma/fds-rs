//! Single-request object uploads.

use super::{insert_header_opt, validate_object};
use super::{UploadBody, UserMetadata};
use crate::client::ClientRequest;
use crate::metadata::WriteMetadata;
use crate::{Client, Result};
use chrono::{DateTime, Utc};
use http::Method;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::time::Duration;

/// Builder for [`Client::put_object`]. It owns the body (bytes or a
/// known-length stream), so it is single-use.
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct PutObject {
    client: Client,
    bucket: String,
    object: String,
    body: UploadBody,
    content_disposition: Option<String>,
    expect: Option<String>,
    metadata: WriteMetadata,
    expires: Option<DateTime<Utc>>,
    timeout: Option<Duration>,
}

crate::de::lenient! {
    /// Result of [`PutObject::send`], also returned by multipart completion.
    ///
    /// Missing or `null` strings read as empty and a missing flag as `false`.
    /// `access_key_id`, `signature` and `expires` describe a download
    /// signature the service issues for the new object (by default valid for
    /// 30 days); it is a credential-bearing value, so never log the whole
    /// response.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct PutObjectOutput {
        pub bucket_name: String,
        pub object_name: String,
        /// The version this write replaced, if the service reported one.
        pub previous_version_id: Option<String>,
        pub access_key_id: String,
        pub signature: String,
        /// When [`signature`](Self::signature) expires, decoded from Unix
        /// epoch milliseconds; not an object TTL.
        pub expires: Option<DateTime<Utc>> => "crate::de::opt_millis",
        pub outside_access: bool,
    }
}

impl PutObject {
    /// User metadata, sent as `x-xiaomi-meta-*` request headers; replaces any
    /// earlier call. See [`UserMetadata`] for the key rules.
    pub fn user_metadata(mut self, metadata: UserMetadata) -> Self {
        self.metadata.user = metadata;
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
    /// `send` fails with an invalid-input error.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// The `Cache-Control` of the object. Empty values are not sent.
    pub fn cache_control(mut self, value: impl Into<String>) -> Self {
        self.metadata.cache_control = Some(value.into());
        self
    }

    /// Empty values are not sent.
    pub fn content_disposition(mut self, value: impl Into<String>) -> Self {
        self.content_disposition = Some(value.into());
        self
    }

    /// The `Content-Encoding` of the object. Empty values are not sent.
    pub fn content_encoding(mut self, value: impl Into<String>) -> Self {
        self.metadata.content_encoding = Some(value.into());
        self
    }

    /// The `Content-Type` of the object. Empty values are not sent. Not
    /// inferred from the object name or body.
    pub fn content_type(mut self, value: impl Into<String>) -> Self {
        self.metadata.content_type = Some(value.into());
        self
    }

    /// Required for stream and file bodies when the client enables MD5
    /// calculation; byte bodies are hashed automatically. Empty values are not
    /// sent.
    pub fn content_md5(mut self, value: impl Into<String>) -> Self {
        self.metadata.content_md5 = Some(value.into());
        self
    }

    /// Empty values are not sent.
    pub fn expect(mut self, value: impl Into<String>) -> Self {
        self.expect = Some(value.into());
        self
    }

    /// PUT `/{bucket}/{object}`, consuming the body without copying or
    /// retrying it.
    pub async fn send(self) -> Result<PutObjectOutput> {
        validate_object(&self.bucket, &self.object)?;
        let mut headers = HeaderMap::new();
        let metadata = &self.metadata;
        for (name, value) in [
            ("cache-control", metadata.cache_control.as_deref()),
            ("content-disposition", self.content_disposition.as_deref()),
            ("content-encoding", metadata.content_encoding.as_deref()),
            ("content-type", metadata.content_type.as_deref()),
            ("content-md5", metadata.content_md5.as_deref()),
            ("expect", self.expect.as_deref()),
        ] {
            insert_header_opt(&mut headers, name, value)?;
        }

        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.headers = headers;
        cr.user_metadata = Some(&self.metadata.user);
        cr.body = Some(self.body);
        if let Some(expires) = self.expires {
            cr.query
                .add("expires", expires.timestamp_millis().to_string());
        }
        self.client.do_json(cr).await
    }
}

impl Client {
    /// Create or overwrite an object with an owned body: bytes, or a
    /// known-length stream or file through [`UploadBody`].
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// use fds::object::UploadBody;
    ///
    /// client
    ///     .put_object("bucket", "hello.txt", b"hello".to_vec())
    ///     .content_type("text/plain")
    ///     .send()
    ///     .await?;
    /// // Keep the file unchanged until the upload completes.
    /// let file = UploadBody::from_file("local.bin").await?;
    /// client.put_object("bucket", "local.bin", file).send().await?;
    /// # Ok(()) }
    /// ```
    pub fn put_object(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
        body: impl Into<UploadBody>,
    ) -> PutObject {
        PutObject {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            body: body.into(),
            content_disposition: None,
            expect: None,
            metadata: WriteMetadata::default(),
            expires: None,
            timeout: None,
        }
    }
}
