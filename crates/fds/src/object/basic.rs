//! Object reads, copies, renames and cache operations.

use super::validate_object;
use crate::client::ClientRequest;
use crate::{ByteRange, Client, FdsError, GetObjectOutput, HttpMethod, ObjectMetadata, Result};
use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};

/// Input of `get_object` (Go `GetObjectRequest`).
#[derive(Debug, Clone, Default)]
pub struct GetObjectRequest {
    pub bucket_name: String,
    pub object_name: String,
    /// `Range` header (e.g. `bytes=0-1023`); `None` → not sent.
    pub range: Option<crate::ByteRange>,
}

/// Input of `copy_object` (Go `CopyObjectRequest`).
#[derive(Debug, Clone, Default)]
pub struct CopyObjectRequest {
    pub source_bucket_name: String,
    pub source_object_name: String,
    pub target_bucket_name: String,
    pub target_object_name: String,
}

/// Validated destination receipt. Additional service fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyObjectResponse {
    pub bucket_name: String,
    pub object_name: String,
}

#[derive(Serialize)]
struct CopyBody<'a> {
    #[serde(rename = "srcBucketName")]
    src_bucket_name: &'a str,
    #[serde(rename = "srcObjectName")]
    src_object_name: &'a str,
}

impl GetObjectRequest {
    pub fn new(bucket: impl Into<String>, object: impl Into<String>) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            range: None,
        }
    }
    pub fn range(mut self, range: ByteRange) -> Self {
        self.range = Some(range);
        self
    }
    pub async fn send(self, client: &Client) -> Result<GetObjectOutput> {
        client.get_object(&self).await
    }
}

impl CopyObjectRequest {
    pub fn new(
        source_bucket: impl Into<String>,
        source_object: impl Into<String>,
        target_bucket: impl Into<String>,
        target_object: impl Into<String>,
    ) -> Self {
        Self {
            source_bucket_name: source_bucket.into(),
            source_object_name: source_object.into(),
            target_bucket_name: target_bucket.into(),
            target_object_name: target_object.into(),
        }
    }
    pub async fn send(self, client: &Client) -> Result<()> {
        client.copy_object(&self).await
    }
    /// Copy and return a fully validated destination receipt.
    pub async fn send_with_response(self, client: &Client) -> Result<CopyObjectResponse> {
        client.copy_object_with_response(&self).await
    }
}

impl Client {
    /// Get an object (GET `/{b}/{o}`, optional Range) as a streaming SDK response.
    /// Use `bytes()` to collect or `copy_to()` to stream to a writer. Status 206
    /// is explicitly accepted only when a Range header was sent.
    pub async fn get_object(&self, req: &GetObjectRequest) -> Result<crate::GetObjectOutput> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let mut headers = HeaderMap::new();
        if let Some(range) = req.range {
            headers.insert(
                "range",
                HeaderValue::from_str(&range.to_string())
                    .map_err(|e| FdsError::invalid(e.to_string()))?,
            );
        }
        let mut cr =
            ClientRequest::new(&req.bucket_name, &req.object_name, HttpMethod::Get).read_only();
        if headers.contains_key("range") {
            cr.accepted_statuses = &[200, 206];
        }
        cr.headers = headers;
        Ok(crate::GetObjectOutput::new(self.do_request(cr).await?))
    }

    /// Whether an object exists (HEAD; 200 → true, 404 → false).
    pub async fn does_object_exist(&self, bucket: &str, object: &str) -> Result<bool> {
        Ok(self.head_object(bucket, object).await?.is_some())
    }

    /// HEAD without a metadata query. 200 returns metadata, 404 returns None;
    /// every other status and transport failure remains an error.
    pub async fn head_object(&self, bucket: &str, object: &str) -> Result<Option<ObjectMetadata>> {
        validate_object(bucket, object)?;
        let mut cr = ClientRequest::new(bucket, object, HttpMethod::Head).read_only();
        cr.accepted_statuses = &[200, 404];
        let resp = self.do_request(cr).await?;
        Ok((resp.status().as_u16() == 200).then(|| ObjectMetadata::from_headers(resp.headers())))
    }

    /// Copy an object (PUT `/{tb}/{to}?cp`, body `{srcBucketName, srcObjectName}`).
    pub async fn copy_object(&self, req: &CopyObjectRequest) -> Result<()> {
        self.copy_object_with_response(req).await.map(|_| ())
    }

    /// Copy with a required matching JSON receipt (maximum 64 KiB).
    /// No automatic retries. Errors after dispatch may mean the destination was
    /// written; reconcile with the service before deciding whether to retry.
    pub async fn copy_object_with_response(
        &self,
        req: &CopyObjectRequest,
    ) -> Result<CopyObjectResponse> {
        validate_object(&req.source_bucket_name, &req.source_object_name)?;
        validate_object(&req.target_bucket_name, &req.target_object_name)?;
        let body = serde_json::to_vec(&CopyBody {
            src_bucket_name: &req.source_bucket_name,
            src_object_name: &req.source_object_name,
        })?;
        let mut cr = ClientRequest::new(
            &req.target_bucket_name,
            &req.target_object_name,
            HttpMethod::Put,
        );
        cr.query.add("cp", "");
        cr.body = Some(Bytes::from(body).into());
        let receipt: CopyObjectResponse = self
            .do_json_limited(cr, 64 * 1024)
            .await
            .map_err(|e| FdsError::CopyOutcomeUnknown(Box::new(e)))?;
        if receipt.bucket_name != req.target_bucket_name
            || receipt.object_name != req.target_object_name
        {
            return Err(FdsError::CopyOutcomeUnknown(Box::new(FdsError::invalid(
                "Copy receipt destination does not match request",
            ))));
        }
        Ok(receipt)
    }

    /// Rename an object (PUT `/{b}/{src}?renameTo=<dst>`).
    pub async fn rename_object(&self, bucket: &str, src: &str, dst: &str) -> Result<()> {
        validate_object(bucket, src)?;
        validate_object(bucket, dst)?;
        let mut cr = ClientRequest::new(bucket, src, HttpMethod::Put);
        cr.query.add("renameTo", dst);
        self.do_discard(cr).await
    }

    /// Restore a deleted object (PUT `/{b}/{o}?restore`).
    ///
    /// This is deleted-object recovery, not archive thawing (`?restoreArchive`).
    pub async fn restore_object(&self, bucket: &str, object: &str) -> Result<()> {
        validate_object(bucket, object)?;
        let mut cr = ClientRequest::new(bucket, object, HttpMethod::Put);
        cr.query.add("restore", "");
        self.do_discard(cr).await
    }

    /// Request asynchronous CDN prefetch (PUT `?prefetch`).
    ///
    /// Success acknowledges the request, not cache readiness. Availability and
    /// propagation depend on the region/CDN; some regions perform no action.
    pub async fn prefetch_object(&self, bucket: &str, object: &str) -> Result<()> {
        self.cache_object(bucket, object, "prefetch").await
    }

    /// Request asynchronous CDN refresh (PUT `?refresh`).
    ///
    /// Success acknowledges the request, not completed invalidation. The caller
    /// must own the object. No automatic retry is performed for this mutation.
    pub async fn refresh_object(&self, bucket: &str, object: &str) -> Result<()> {
        self.cache_object(bucket, object, "refresh").await
    }

    async fn cache_object(&self, bucket: &str, object: &str, operation: &str) -> Result<()> {
        validate_object(bucket, object)?;
        let mut cr = ClientRequest::new(bucket, object, HttpMethod::Put);
        // Official REST traces and Java SDK use query flags, despite the
        // generated parameter table incorrectly listing them as headers.
        cr.query.add(operation, "");
        cr.headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=UTF-8"),
        );
        // The documented REST response is empty. Newer Java clients consume a
        // numeric body; neither shape is needed to acknowledge this operation.
        self.do_discard(cr).await
    }
}
