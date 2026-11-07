//! Low-level multipart requests: create, upload part, complete and abort.
//! Re-exported from [`crate::multipart`]; see there for the managed file upload.

use super::{insert_header_opt, validate_object};
use super::{PutObjectOutput, UploadBody};
use crate::client::ClientRequest;
use crate::metadata::{write_metadata_methods, WriteMetadata};
use crate::{Client, Error, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::Method;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU16;
use std::time::Duration;

/// The largest part number FDS accepts.
pub const MAX_PART_NUMBER: u16 = 10_000;

crate::de::lenient! {
    /// The initiation response; only the upload ID is trusted.
    #[derive(Debug, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub(crate) struct InitMultipartUploadResponse {
        pub upload_id: String,
    }
}

/// Result of [`UploadPart::send`]: the part receipt, sent back verbatim on
/// completion (`{"partNumber":..,"etag":..,"partSize":..}`).
///
/// Every key is required. A part number outside `1..=65535` or a negative
/// size is a protocol violation; the managed upload additionally checks that
/// each receipt matches the part it sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadPartOutput {
    /// The part number, the same type [`Client::upload_part`] takes.
    pub part_number: NonZeroU16,
    pub etag: String,
    /// Size in bytes.
    pub part_size: u64,
}

fn validate_upload(bucket: &str, object: &str, upload_id: &str) -> Result<()> {
    validate_object(bucket, object)?;
    if upload_id.is_empty() {
        return Err(Error::invalid_input("upload ID must be non-empty"));
    }
    Ok(())
}

/// Builder for [`Client::create_multipart_upload`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct CreateMultipartUpload {
    client: Client,
    bucket: String,
    object: String,
    metadata: WriteMetadata,
    timeout: Option<Duration>,
}

/// The typed headers and user metadata are sent as request headers with the
/// initiation.
impl CreateMultipartUpload {
    write_metadata_methods!("Empty values are not sent.");

    /// Replace every typed header and the user metadata at once.
    pub(crate) fn write_metadata(mut self, metadata: WriteMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    timeout_option!();

    /// PUT `/{bucket}/{object}?uploads` once and return the session.
    ///
    /// The session is bound to the requested bucket and object, never to
    /// names echoed by the service. A response without an upload ID is
    /// [`ErrorKind::OutcomeUnknown`](crate::ErrorKind::OutcomeUnknown): an
    /// upload may have been created. Never retry initiation blindly; a failure
    /// after the request was sent can leave an upload whose ID was not
    /// received.
    pub async fn send(self) -> Result<MultipartUploadSession> {
        validate_object(&self.bucket, &self.object)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.query.add("uploads", "");
        self.metadata.insert_system_headers(&mut cr.headers)?;
        cr.user_metadata = Some(&self.metadata.user);
        let response: InitMultipartUploadResponse = self.client.do_json(cr).await?;
        if response.upload_id.is_empty() {
            return Err(
                Error::protocol("multipart initialization returned no upload ID")
                    .into_outcome_unknown(
                        "multipart initialization outcome unknown; an upload may have been created",
                    ),
            );
        }
        Ok(MultipartUploadSession {
            client: self.client,
            bucket: self.bucket,
            object: self.object,
            upload_id: response.upload_id,
        })
    }
}

/// An initiated multipart upload: the bucket, object and upload ID, plus the
/// [`Client`] that sends the follow-up requests.
///
/// Created by [`CreateMultipartUpload::send`]. Each method returns the same
/// builder as the matching [`Client`] method with the session's identity
/// filled in. Nothing is sent implicitly, also not on drop: persist
/// [`upload_id`](Self::upload_id) if the upload may need to be completed or
/// aborted later, from another process through
/// [`Client::upload_part`], [`Client::complete_multipart_upload`] and
/// [`Client::abort_multipart_upload`].
#[derive(Debug, Clone)]
pub struct MultipartUploadSession {
    client: Client,
    bucket: String,
    object: String,
    upload_id: String,
}

impl MultipartUploadSession {
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn object(&self) -> &str {
        &self.object
    }

    pub fn upload_id(&self) -> &str {
        &self.upload_id
    }

    /// Upload one part; see [`Client::upload_part`].
    pub fn upload_part(&self, part_number: NonZeroU16, body: impl Into<UploadBody>) -> UploadPart {
        self.client.upload_part(
            &self.bucket,
            &self.object,
            &self.upload_id,
            part_number,
            body,
        )
    }

    /// Complete with the confirmed part receipts; see
    /// [`Client::complete_multipart_upload`].
    pub fn complete(
        &self,
        parts: impl IntoIterator<Item = UploadPartOutput>,
    ) -> CompleteMultipartUpload {
        self.client
            .complete_multipart_upload(&self.bucket, &self.object, &self.upload_id, parts)
    }

    /// Abort the upload; see [`Client::abort_multipart_upload`].
    pub fn abort(&self) -> AbortMultipartUpload {
        self.client
            .abort_multipart_upload(&self.bucket, &self.object, &self.upload_id)
    }
}

/// Builder for [`Client::upload_part`]. It owns the body, so it is single-use.
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct UploadPart {
    client: Client,
    bucket: String,
    object: String,
    upload_id: String,
    part_number: NonZeroU16,
    body: UploadBody,
    content_md5: Option<String>,
    timeout: Option<Duration>,
    retry: crate::RetryMode,
}

impl UploadPart {
    /// Send this `Content-MD5` value instead of letting the client compute
    /// one (see `ClientBuilder::enable_md5_calculate`). Empty is not sent.
    pub fn content_md5(mut self, value: impl Into<String>) -> Self {
        self.content_md5 = Some(value.into());
        self
    }

    timeout_option!();

    retry_option!();

    /// PUT `/{bucket}/{object}?uploadId=..&partNumber=..`, consuming the body
    /// without copying it. The part number must not exceed
    /// [`MAX_PART_NUMBER`]; the upload ID must be non-empty.
    ///
    /// An idempotent write (the same part number with the same bytes
    /// replaces the part): a bytes body is retried when the client's
    /// [`RetryPolicy`](crate::RetryPolicy) enables idempotent writes or
    /// [`retry`](Self::retry) says so; a stream or file body is sent once.
    pub async fn send(self) -> Result<UploadPartOutput> {
        validate_upload(&self.bucket, &self.object, &self.upload_id)?;
        if self.part_number.get() > MAX_PART_NUMBER {
            return Err(Error::invalid_input("part number must be at most 10000"));
        }
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::PUT)
            .idempotent(self.retry)
            .with_timeout(self.timeout)?;
        cr.query.add("uploadId", &self.upload_id);
        cr.query.add("partNumber", self.part_number.to_string());
        insert_header_opt(&mut cr.headers, "content-md5", self.content_md5.as_deref())?;
        cr.body = Some(self.body);
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::complete_multipart_upload`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct CompleteMultipartUpload {
    client: Client,
    bucket: String,
    object: String,
    upload_id: String,
    parts: Vec<UploadPartOutput>,
    metadata: WriteMetadata,
    expires: Option<DateTime<Utc>>,
    timeout: Option<Duration>,
}

/// The typed headers and user metadata are sent as request headers with the
/// completion.
impl CompleteMultipartUpload {
    write_metadata_methods!("Empty values are not sent.");

    /// Replace every typed header and the user metadata at once.
    pub(crate) fn write_metadata(mut self, metadata: WriteMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Response-signature expiration, not object TTL.
    pub fn expires(mut self, expires: DateTime<Utc>) -> Self {
        self.expires = Some(expires);
        self
    }

    timeout_option!();

    /// PUT `/{bucket}/{object}?uploadId=..` with body
    /// `{"uploadPartResultList":[...]}`, the receipts in the given order.
    /// Never retried; reconcile before repeating a completion whose outcome
    /// is unknown.
    pub async fn send(self) -> Result<PutObjectOutput> {
        validate_upload(&self.bucket, &self.object, &self.upload_id)?;
        #[derive(Serialize)]
        struct Parts<'a> {
            #[serde(rename = "uploadPartResultList")]
            parts: &'a [UploadPartOutput],
        }
        let body = serde_json::to_vec(&Parts { parts: &self.parts }).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.query.add("uploadId", &self.upload_id);
        if let Some(expires) = self.expires {
            cr.query
                .add("expires", expires.timestamp_millis().to_string());
        }
        self.metadata.insert_system_headers(&mut cr.headers)?;
        cr.user_metadata = Some(&self.metadata.user);
        cr.body = Some(Bytes::from(body).into());
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::abort_multipart_upload`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct AbortMultipartUpload {
    client: Client,
    bucket: String,
    object: String,
    upload_id: String,
    timeout: Option<Duration>,
}

impl AbortMultipartUpload {
    timeout_option!();

    /// DELETE `/{bucket}/{object}?uploadId=..`. Never retried.
    pub async fn send(self) -> Result<()> {
        validate_upload(&self.bucket, &self.object, &self.upload_id)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::DELETE)
            .with_timeout(self.timeout)?;
        cr.query.add("uploadId", &self.upload_id);
        self.client.do_discard(cr).await
    }
}

impl Client {
    /// Initiate a multipart upload; `send` returns the
    /// [`MultipartUploadSession`] that uploads, completes or aborts it. For a
    /// local file, [`Client::upload_file`] runs the whole upload.
    pub fn create_multipart_upload(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> CreateMultipartUpload {
        CreateMultipartUpload {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            metadata: WriteMetadata::default(),
            timeout: None,
        }
    }

    /// Upload one part of the upload `upload_id`, for example when resuming
    /// from a persisted upload ID in another process. Within one process the
    /// same builder comes from [`MultipartUploadSession::upload_part`].
    ///
    /// Part numbers start at 1 and must not exceed [`MAX_PART_NUMBER`]
    /// (checked by `send`).
    pub fn upload_part(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
        upload_id: impl Into<String>,
        part_number: NonZeroU16,
        body: impl Into<UploadBody>,
    ) -> UploadPart {
        UploadPart {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            upload_id: upload_id.into(),
            part_number,
            body: body.into(),
            content_md5: None,
            timeout: None,
            retry: crate::RetryMode::Policy,
        }
    }

    /// Complete the upload `upload_id` from the caller's confirmed part
    /// receipts, sent in the given order (FDS expects ascending part
    /// numbers). Also available as [`MultipartUploadSession::complete`].
    pub fn complete_multipart_upload(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
        upload_id: impl Into<String>,
        parts: impl IntoIterator<Item = UploadPartOutput>,
    ) -> CompleteMultipartUpload {
        CompleteMultipartUpload {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            upload_id: upload_id.into(),
            parts: parts.into_iter().collect(),
            metadata: WriteMetadata::default(),
            expires: None,
            timeout: None,
        }
    }

    /// Abort the upload `upload_id` explicitly. Also available as
    /// [`MultipartUploadSession::abort`].
    pub fn abort_multipart_upload(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
        upload_id: impl Into<String>,
    ) -> AbortMultipartUpload {
        AbortMultipartUpload {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            upload_id: upload_id.into(),
            timeout: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InitMultipartUploadResponse;

    // Lenient: `null`, missing and unknown keys decode; only the upload ID is kept.
    #[test]
    fn init_response_is_lenient() {
        for body in [
            r#"{"bucketName":"b","objectName":"o","uploadId":"u","x":1}"#,
            r#"{"uploadId":"u","bucketName":null}"#,
        ] {
            let parsed: InitMultipartUploadResponse = serde_json::from_str(body).unwrap();
            assert_eq!(parsed.upload_id, "u");
        }
        for body in [r#"{}"#, r#"{"uploadId":null}"#] {
            let parsed: InitMultipartUploadResponse = serde_json::from_str(body).unwrap();
            assert_eq!(parsed.upload_id, "");
        }
    }
}
