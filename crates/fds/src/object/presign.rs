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

/// Input of `generate_presigned_url`.
#[derive(Debug, Clone)]
pub struct GeneratePresignedURLRequest {
    pub cdn: bool,
    pub bucket_name: String,
    pub object_name: String,
    pub method: Method,
    pub expiration: DateTime<Utc>,
    pub metadata: Option<ObjectMetadata>,
}

impl GeneratePresignedURLRequest {
    /// Expiration is an absolute UTC instant, encoded as milliseconds on the wire.
    pub fn new(
        bucket: impl Into<String>,
        object: impl Into<String>,
        method: Method,
        expiration: DateTime<Utc>,
    ) -> Self {
        Self {
            cdn: false,
            bucket_name: bucket.into(),
            object_name: object.into(),
            method,
            expiration,
            metadata: None,
        }
    }
    /// Compute expiration once at construction; zero/overflowing lifetimes fail locally.
    pub fn expires_in(
        bucket: impl Into<String>,
        object: impl Into<String>,
        method: Method,
        lifetime: Duration,
    ) -> Result<Self> {
        if lifetime.as_millis() == 0 {
            return Err(Error::invalid_input(
                "presign lifetime must be at least one millisecond",
            ));
        }
        let delta = chrono::Duration::from_std(lifetime)
            .map_err(|_| Error::invalid_input("presign lifetime overflow"))?;
        let expiration = Utc::now()
            .checked_add_signed(delta)
            .ok_or_else(|| Error::invalid_input("presign expiration overflow"))?;
        Ok(Self::new(bucket, object, method, expiration))
    }
    pub fn cdn(mut self, value: bool) -> Self {
        self.cdn = value;
        self
    }
    pub fn generate(self, client: &Client) -> Result<String> {
        client.generate_presigned_url(&self)
    }

    pub fn metadata(mut self, value: ObjectMetadata) -> Self {
        self.metadata = Some(value);
        self
    }
}

impl Client {
    /// Build an unsigned URL. An empty object selects the bucket URL; empty
    /// bucket and object select the endpoint root. No network request occurs.
    pub fn generate_absolute_object_url(&self, bucket: &str, object: &str) -> Result<String> {
        Ok(self.build_request_url(bucket, object, "", false)?.into())
    }

    /// Generate a presigned URL.
    ///
    /// Returns the exact `String`: `Expires` is a millisecond timestamp and
    /// drives the signature's Date row; `Signature` is appended after the
    /// sorted query and is NOT URL-encoded.
    pub fn generate_presigned_url(&self, req: &GeneratePresignedURLRequest) -> Result<String> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let (access_id, access_secret) = self.access_keys()?;
        let base = self.build_request_url(&req.bucket_name, &req.object_name, "", req.cdn)?;

        let expires_ms = req.expiration.timestamp_millis();
        let mut params = QueryValues::new();
        if req.method == Method::HEAD {
            params.add("metadata", "");
        }
        params.add(HTTP_HEADER_GALAXY_ACCESS_KEY_ID, access_id);
        params.add(HTTP_HEADER_EXPIRES, expires_ms.to_string());
        let query = params.encode();

        // base has no query; hand-append (do not use Url::set_query — it would
        // re-encode the already-encoded query).
        let with_query = format!("{}?{}", base.as_str(), query);

        let mut headers = HeaderMap::new();
        if let Some(metadata) = &req.metadata {
            metadata.append_to_headers(&mut headers)?;
        }
        let sig = auth::sign(access_secret, req.method.as_str(), &with_query, &headers)?;

        // Append &Signature=<sig> raw (unsorted, un-encoded).
        Ok(format!("{}&{}={}", with_query, HTTP_HEADER_SIGNATURE, sig))
    }
}
