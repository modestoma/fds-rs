//! Manual multipart requests and session helpers. See crate::multipart for managed file uploads.

use super::{insert_header_opt, validate_object};
use super::{ObjectMetadata, PutObjectResponse, UploadBody};
use crate::client::ClientRequest;
use crate::{Client, Error, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::Method;
use serde::{Deserialize, Serialize};

/// Input of `init_multipart_upload`.
#[derive(Debug, Clone, Default)]
pub struct InitMultipartUploadRequest {
    pub bucket_name: String,
    pub object_name: String,
    pub metadata: Option<ObjectMetadata>,
}

crate::de::lenient! {
    /// Result of `init_multipart_upload`.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct InitMultipartUploadResponse {
        pub bucket_name: String,
        pub object_name: String,
        pub upload_id: String,
    }
}

/// Single-use multipart part request; part numbers start at 1.
#[derive(Debug)]
pub struct UploadPartRequest {
    bucket_name: String,
    object_name: String,
    upload_id: String,
    part_number: i32,
    body: crate::object::UploadBody,
    content_md5: Option<String>,
}

/// Result of `upload_part`, sent back verbatim on completion. Every key is
/// required.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadPartResponse {
    pub part_number: i32,
    pub etag: String,
    pub part_size: i64,
}

/// Input of `complete_multipart_upload`.
#[derive(Debug, Clone, Default)]
pub struct CompleteMultipartUploadRequest {
    pub bucket_name: String,
    pub object_name: String,
    pub upload_id: String,
    pub upload_parts: Vec<UploadPartResponse>,
    pub metadata: Option<ObjectMetadata>,
    pub expires: Option<DateTime<Utc>>,
}

impl InitMultipartUploadRequest {
    pub fn new(bucket: impl Into<String>, object: impl Into<String>) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            metadata: None,
        }
    }
    pub async fn send(self, client: &Client) -> Result<InitMultipartUploadResponse> {
        client.init_multipart_upload(&self).await
    }

    pub fn metadata(mut self, value: ObjectMetadata) -> Self {
        self.metadata = Some(value);
        self
    }
}

impl UploadPartRequest {
    pub fn new(
        bucket: impl Into<String>,
        object: impl Into<String>,
        upload_id: impl Into<String>,
        part_number: i32,
        body: impl Into<UploadBody>,
    ) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            upload_id: upload_id.into(),
            part_number,
            body: body.into(),
            content_md5: None,
        }
    }
    pub fn content_md5(mut self, value: impl Into<String>) -> Self {
        self.content_md5 = Some(value.into());
        self
    }
    pub async fn send(self, client: &Client) -> Result<UploadPartResponse> {
        client.upload_part(self).await
    }
}

impl CompleteMultipartUploadRequest {
    pub fn new(
        bucket: impl Into<String>,
        object: impl Into<String>,
        upload_id: impl Into<String>,
        upload_parts: impl IntoIterator<Item = UploadPartResponse>,
    ) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            upload_id: upload_id.into(),
            upload_parts: upload_parts.into_iter().collect(),
            metadata: None,
            expires: None,
        }
    }
    pub async fn send(self, client: &Client) -> Result<PutObjectResponse> {
        client.complete_multipart_upload(&self).await
    }

    pub fn metadata(mut self, value: ObjectMetadata) -> Self {
        self.metadata = Some(value);
        self
    }

    /// Response-signature expiration, not object TTL.
    pub fn expires(mut self, expires: DateTime<Utc>) -> Self {
        self.expires = Some(expires);
        self
    }
}

impl InitMultipartUploadResponse {
    /// Build a single-use part request bound to this upload session.
    pub fn part(
        &self,
        number: i32,
        body: impl Into<crate::object::UploadBody>,
    ) -> UploadPartRequest {
        UploadPartRequest::new(
            &self.bucket_name,
            &self.object_name,
            &self.upload_id,
            number,
            body,
        )
    }
    /// Build completion from the caller's confirmed part receipts.
    pub fn complete(
        &self,
        parts: impl IntoIterator<Item = UploadPartResponse>,
    ) -> CompleteMultipartUploadRequest {
        CompleteMultipartUploadRequest::new(
            &self.bucket_name,
            &self.object_name,
            &self.upload_id,
            parts,
        )
    }
    /// Explicitly abort this session; no implicit cleanup or retries.
    pub async fn abort(&self, client: &Client) -> Result<()> {
        client.abort_multipart_upload(self).await
    }
}

impl Client {
    /// Start a multipart upload (PUT `/{b}/{o}?uploads`).
    pub async fn init_multipart_upload(
        &self,
        req: &InitMultipartUploadRequest,
    ) -> Result<InitMultipartUploadResponse> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, Method::PUT);
        cr.query.add("uploads", "");
        cr.metadata = req.metadata.as_ref();
        self.do_json(cr).await
    }

    /// Upload one part, consuming its body without copying or retrying it.
    pub async fn upload_part(&self, req: UploadPartRequest) -> Result<UploadPartResponse> {
        validate_object(&req.bucket_name, &req.object_name)?;
        if req.upload_id.is_empty() || req.part_number <= 0 {
            return Err(Error::invalid_input(
                "upload ID must be non-empty and part number positive",
            ));
        }
        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, Method::PUT);
        cr.query.add("uploadId", &req.upload_id);
        cr.query.add("partNumber", req.part_number.to_string());
        insert_header_opt(&mut cr.headers, "content-md5", req.content_md5.as_deref())?;
        cr.body = Some(req.body);
        self.do_json(cr).await
    }

    /// Complete a multipart upload (PUT `/{b}/{o}?uploadId`, body =
    /// `{"uploadPartResultList":[...]}`).
    pub async fn complete_multipart_upload(
        &self,
        req: &CompleteMultipartUploadRequest,
    ) -> Result<PutObjectResponse> {
        validate_object(&req.bucket_name, &req.object_name)?;
        if req.upload_id.is_empty() {
            return Err(Error::invalid_input("upload ID must be non-empty"));
        }
        #[derive(Serialize)]
        struct Parts<'a> {
            #[serde(rename = "uploadPartResultList")]
            parts: &'a [UploadPartResponse],
        }
        let body = serde_json::to_vec(&Parts {
            parts: &req.upload_parts,
        })
        .map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, Method::PUT);
        cr.query.add("uploadId", &req.upload_id);
        if let Some(expires) = req.expires {
            cr.query
                .add("expires", expires.timestamp_millis().to_string());
        }
        cr.metadata = req.metadata.as_ref();
        cr.body = Some(Bytes::from(body).into());
        self.do_json(cr).await
    }

    /// Abort a multipart upload (DELETE `/{b}/{o}?uploadId`).
    pub async fn abort_multipart_upload(&self, init: &InitMultipartUploadResponse) -> Result<()> {
        validate_object(&init.bucket_name, &init.object_name)?;
        if init.upload_id.is_empty() {
            return Err(Error::invalid_input("upload ID must be non-empty"));
        }
        let mut cr = ClientRequest::new(&init.bucket_name, &init.object_name, Method::DELETE);
        cr.query.add("uploadId", &init.upload_id);
        self.do_discard(cr).await
    }
}
