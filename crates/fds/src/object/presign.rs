//! Absolute and presigned object URLs.

use super::validate_object;
use super::ObjectMetadata;
use crate::auth;
use crate::constant::HTTP_HEADER_EXPIRES;
use crate::constant::HTTP_HEADER_GALAXY_ACCESS_KEY_ID;
use crate::constant::HTTP_HEADER_SIGNATURE;
use crate::encoding::QueryValues;
use crate::{Client, Error, Result};
use chrono::{DateTime, Utc};
use http::Method;
use reqwest::header::HeaderMap;
use std::time::Duration;

/// Builder for [`Client::presign_object`]. Presigning is local: `build`
/// signs with the client's access key and makes no request, so this builder
/// has no `timeout` or `send`.
#[derive(Debug)]
#[must_use = "a presign builder does nothing until `build` is called"]
pub struct PresignObject {
    client: Client,
    bucket: String,
    object: String,
    method: Method,
    expiration: Option<Expiration>,
    cdn: bool,
    metadata: Option<ObjectMetadata>,
}

#[derive(Debug, Clone, Copy)]
enum Expiration {
    At(DateTime<Utc>),
    In(Duration),
}

impl PresignObject {
    /// The HTTP method the URL is signed for. Defaults to `GET`; a `HEAD`
    /// URL also signs the `metadata` subresource.
    pub fn method(mut self, method: Method) -> Self {
        self.method = method;
        self
    }

    /// Expire at an absolute UTC instant, encoded as milliseconds on the wire.
    /// Either this or [`expires_in`](Self::expires_in) is required.
    pub fn expires_at(mut self, expiration: DateTime<Utc>) -> Self {
        self.expiration = Some(Expiration::At(expiration));
        self
    }

    /// Expire this long after [`build`](Self::build) is called. The lifetime
    /// must be at least one millisecond and must not overflow; otherwise
    /// `build` fails with an invalid-input error.
    pub fn expires_in(mut self, lifetime: Duration) -> Self {
        self.expiration = Some(Expiration::In(lifetime));
        self
    }

    /// Use the client's CDN endpoint as the URL host; `build` fails if the
    /// client has none configured.
    pub fn cdn(mut self) -> Self {
        self.cdn = true;
        self
    }

    /// Metadata headers the request will send; they are part of the signature.
    pub fn metadata(mut self, metadata: ObjectMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Sign and return the URL.
    ///
    /// `Expires` is a millisecond timestamp and drives the signature's Date
    /// row; `Signature` is appended after the sorted query and is NOT
    /// URL-encoded.
    pub fn build(self) -> Result<String> {
        // Check the lifetime first, then the names, then the credentials.
        let expiration = match self.expiration {
            Some(Expiration::At(at)) => at,
            Some(Expiration::In(lifetime)) => expires_after(lifetime)?,
            None => {
                return Err(Error::invalid_input(
                    "presign expiration is required: call expires_at or expires_in",
                ))
            }
        };
        validate_object(&self.bucket, &self.object)?;
        let (access_id, access_secret) = self.client.access_keys()?;
        let base = self
            .client
            .build_request_url(&self.bucket, &self.object, "", self.cdn)?;

        let expires_ms = expiration.timestamp_millis();
        let mut params = QueryValues::new();
        if self.method == Method::HEAD {
            params.add("metadata", "");
        }
        params.add(HTTP_HEADER_GALAXY_ACCESS_KEY_ID, access_id);
        params.add(HTTP_HEADER_EXPIRES, expires_ms.to_string());
        let query = params.encode();

        // base has no query; hand-append (do not use Url::set_query — it would
        // re-encode the already-encoded query).
        let with_query = format!("{}?{}", base.as_str(), query);

        let mut headers = HeaderMap::new();
        if let Some(metadata) = &self.metadata {
            metadata.append_to_headers(&mut headers)?;
        }
        let sig = auth::sign(access_secret, self.method.as_str(), &with_query, &headers)?;

        // Append &Signature=<sig> raw (unsorted, un-encoded).
        Ok(format!("{}&{}={}", with_query, HTTP_HEADER_SIGNATURE, sig))
    }
}

fn expires_after(lifetime: Duration) -> Result<DateTime<Utc>> {
    if lifetime.as_millis() == 0 {
        return Err(Error::invalid_input(
            "presign lifetime must be at least one millisecond",
        ));
    }
    let delta = chrono::Duration::from_std(lifetime)
        .map_err(|_| Error::invalid_input("presign lifetime overflow"))?;
    Utc::now()
        .checked_add_signed(delta)
        .ok_or_else(|| Error::invalid_input("presign expiration overflow"))
}

impl Client {
    /// Build an unsigned URL. An empty object selects the bucket URL; empty
    /// bucket and object select the endpoint root. No network request occurs.
    pub fn absolute_object_url(&self, bucket: &str, object: &str) -> Result<String> {
        Ok(self.build_request_url(bucket, object, "", false)?.into())
    }

    /// Presign a URL for an object with the client's access key. No network
    /// request occurs.
    ///
    /// ```no_run
    /// # fn example(client: &fds::Client) -> fds::Result<()> {
    /// use http::Method;
    /// use std::time::Duration;
    ///
    /// let url = client
    ///     .presign_object("bucket", "key")
    ///     .method(Method::GET)
    ///     .expires_in(Duration::from_secs(3600))
    ///     .cdn()
    ///     .build()?;
    /// # let _ = url; Ok(()) }
    /// ```
    pub fn presign_object(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> PresignObject {
        PresignObject {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            method: Method::GET,
            expiration: None,
            cdn: false,
            metadata: None,
        }
    }
}
