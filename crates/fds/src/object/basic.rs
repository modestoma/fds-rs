//! Object reads, existence checks, copies, renames, restores and cache
//! operations.

use super::validate_object;
use super::{ByteRange, GetObjectOutput, ObjectMetadata};
use crate::client::ClientRequest;
use crate::{Client, Error, Result};
use bytes::Bytes;
use http::Method;
use reqwest::header::{HeaderValue, RANGE};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const COPY_UNKNOWN: &str =
    "copy outcome unknown; the destination may have been written, do not retry blindly";

/// Builder for [`Client::get_object`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct GetObject {
    client: Client,
    bucket: String,
    object: String,
    range: Option<ByteRange>,
    timeout: Option<Duration>,
}

impl GetObject {
    /// Send a `Range` header; the service may answer 206 or ignore it with 200.
    pub fn range(mut self, range: ByteRange) -> Self {
        self.range = Some(range);
        self
    }

    /// Set a total deadline for this request only, overriding the client-wide
    /// [`request_timeout`](crate::ClientBuilder::request_timeout) whether that is
    /// set or not.
    ///
    /// The deadline runs from connecting until the response body has been fully
    /// consumed, so it also bounds [`GetObjectOutput::chunk`],
    /// [`GetObjectOutput::copy_to`] and [`GetObjectOutput::bytes`]: a download
    /// that is still progressing fails with a timeout once it passes. With
    /// retries enabled it applies to each attempt separately. The client's
    /// connect and read idle timeouts still apply. Must be nonzero and
    /// representable; otherwise `send` fails with an invalid-input error.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// GET `/{bucket}/{object}` as a streaming response. Use
    /// [`GetObjectOutput::bytes`] to collect or [`GetObjectOutput::copy_to`] to
    /// stream to a writer. Status 206 is accepted only when a range was set.
    pub async fn send(self) -> Result<GetObjectOutput> {
        validate_object(&self.bucket, &self.object)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        if let Some(range) = self.range {
            cr.headers
                .insert(RANGE, crate::client::header_value(&range.to_string())?);
            cr.accepted_statuses = &[200, 206];
        }
        Ok(GetObjectOutput::new(self.client.do_request(cr).await?))
    }
}

object_builder! {
    /// Builder for [`Client::head_object`].
    HeadObject
}

impl HeadObject {
    /// HEAD `/{bucket}/{object}` without a query. 200 returns the metadata
    /// parsed from the response headers, 404 returns `None`; every other
    /// status and transport failure is an error.
    pub async fn send(self) -> Result<Option<ObjectMetadata>> {
        head(&self.client, &self.bucket, &self.object, self.timeout).await
    }
}

object_builder! {
    /// Builder for [`Client::object_exists`].
    ObjectExists
}

impl ObjectExists {
    /// The same HEAD request as [`HeadObject::send`]: 200 is `true`, 404 is
    /// `false`, anything else is an error.
    pub async fn send(self) -> Result<bool> {
        Ok(head(&self.client, &self.bucket, &self.object, self.timeout)
            .await?
            .is_some())
    }
}

/// The validated, read-only (retryable) HEAD request of [`HeadObject`],
/// [`ObjectExists`] and the file download; it accepts only 200.
pub(crate) fn head_request<'a>(
    bucket: &'a str,
    object: &'a str,
    timeout: Option<Duration>,
) -> Result<ClientRequest<'a>> {
    validate_object(bucket, object)?;
    ClientRequest::new(bucket, object, Method::HEAD)
        .read_only()
        .with_timeout(timeout)
}

async fn head(
    client: &Client,
    bucket: &str,
    object: &str,
    timeout: Option<Duration>,
) -> Result<Option<ObjectMetadata>> {
    let mut cr = head_request(bucket, object, timeout)?;
    cr.accepted_statuses = &[200, 404];
    let resp = client.do_request(cr).await?;
    Ok((resp.status().as_u16() == 200).then(|| ObjectMetadata::from_headers(resp.headers())))
}

/// Builder for [`Client::copy_object`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct CopyObject {
    client: Client,
    source_bucket: String,
    source_object: String,
    target_bucket: String,
    target_object: String,
    timeout: Option<Duration>,
}

/// Validated destination receipt of [`Client::copy_object`]. Additional
/// service fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyObjectOutput {
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

impl CopyObject {
    timeout_option!();

    /// PUT `/{target bucket}/{target object}?cp` with body
    /// `{srcBucketName, srcObjectName}`, and return the destination receipt
    /// (a required JSON body of at most 64 KiB that must name the target).
    ///
    /// No automatic retries. A failure whose [`Dispatch`](crate::Dispatch) is
    /// `NotSent` (local validation, signing, connection) is returned as is.
    /// Every failure after the request may have reached the service, including
    /// an error status, a missing or mismatched receipt and a broken response
    /// body, is [`ErrorKind::OutcomeUnknown`](crate::ErrorKind::OutcomeUnknown):
    /// the destination may have been written. `status()` and the source chain
    /// keep the underlying cause; reconcile before deciding whether to retry.
    pub async fn send(self) -> Result<CopyObjectOutput> {
        validate_object(&self.source_bucket, &self.source_object)?;
        validate_object(&self.target_bucket, &self.target_object)?;
        let body = serde_json::to_vec(&CopyBody {
            src_bucket_name: &self.source_bucket,
            src_object_name: &self.source_object,
        })
        .map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(&self.target_bucket, &self.target_object, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.query.add("cp", "");
        cr.body = Some(Bytes::from(body).into());
        let receipt: CopyObjectOutput =
            self.client
                .do_json_limited(cr, 64 * 1024)
                .await
                .map_err(|e| match e.dispatch() {
                    crate::Dispatch::NotSent => e,
                    _ => e.into_outcome_unknown(COPY_UNKNOWN),
                })?;
        if receipt.bucket_name != self.target_bucket || receipt.object_name != self.target_object {
            return Err(
                Error::protocol("copy receipt destination does not match request")
                    .into_outcome_unknown(COPY_UNKNOWN),
            );
        }
        Ok(receipt)
    }
}

/// Builder for [`Client::rename_object`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct RenameObject {
    client: Client,
    bucket: String,
    source: String,
    target: String,
    timeout: Option<Duration>,
}

impl RenameObject {
    timeout_option!();

    /// PUT `/{bucket}/{source}?renameTo={target}`.
    pub async fn send(self) -> Result<()> {
        validate_object(&self.bucket, &self.source)?;
        validate_object(&self.bucket, &self.target)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.source, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.query.add("renameTo", &self.target);
        self.client.do_discard(cr).await
    }
}

object_builder! {
    /// Builder for [`Client::restore_object`].
    RestoreObject
}

impl RestoreObject {
    /// PUT `/{bucket}/{object}?restore`: deleted-object recovery, not archive
    /// thawing (`?restoreArchive`).
    pub async fn send(self) -> Result<()> {
        validate_object(&self.bucket, &self.object)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.query.add("restore", "");
        self.client.do_discard(cr).await
    }
}

object_builder! {
    /// Builder for [`Client::prefetch_object`].
    PrefetchObject
}

impl PrefetchObject {
    /// Request asynchronous CDN prefetch (PUT `?prefetch`).
    ///
    /// Success acknowledges the request, not cache readiness. Availability and
    /// propagation depend on the region/CDN; some regions perform no action.
    pub async fn send(self) -> Result<()> {
        cache_object(
            &self.client,
            &self.bucket,
            &self.object,
            "prefetch",
            self.timeout,
        )
        .await
    }
}

object_builder! {
    /// Builder for [`Client::refresh_object`].
    RefreshObject
}

impl RefreshObject {
    /// Request asynchronous CDN refresh (PUT `?refresh`).
    ///
    /// Success acknowledges the request, not completed invalidation. The caller
    /// must own the object. No automatic retry is performed for this mutation.
    pub async fn send(self) -> Result<()> {
        cache_object(
            &self.client,
            &self.bucket,
            &self.object,
            "refresh",
            self.timeout,
        )
        .await
    }
}

async fn cache_object(
    client: &Client,
    bucket: &str,
    object: &str,
    operation: &str,
    timeout: Option<Duration>,
) -> Result<()> {
    validate_object(bucket, object)?;
    let mut cr = ClientRequest::new(bucket, object, Method::PUT).with_timeout(timeout)?;
    // Official REST traces and Java SDK use query flags, despite the
    // generated parameter table incorrectly listing them as headers.
    cr.query.add(operation, "");
    cr.headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=UTF-8"),
    );
    // The documented REST response is empty. Newer Java clients consume a
    // numeric body; neither shape is needed to acknowledge this operation.
    client.do_discard(cr).await
}

impl Client {
    /// Get an object (GET `/{bucket}/{object}`) as a streaming response.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// let bytes = client.get_object("bucket", "key").send().await?.bytes().await?;
    /// # let _ = bytes; Ok(()) }
    /// ```
    pub fn get_object(&self, bucket: impl Into<String>, object: impl Into<String>) -> GetObject {
        GetObject {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            range: None,
            timeout: None,
        }
    }

    /// HEAD an object without a query; `send` returns `None` for 404.
    ///
    /// Unlike [`get_object_metadata`](Self::get_object_metadata) (GET
    /// `?metadata`, where 404 is an error), a missing object is not an error.
    pub fn head_object(&self, bucket: impl Into<String>, object: impl Into<String>) -> HeadObject {
        HeadObject::new(self, bucket, object)
    }

    /// Whether an object exists: the [`head_object`](Self::head_object)
    /// request, reduced to 200 → `true`, 404 → `false`.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// if client.object_exists("bucket", "key").send().await? {
    ///     // ...
    /// }
    /// # Ok(()) }
    /// ```
    pub fn object_exists(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> ObjectExists {
        ObjectExists::new(self, bucket, object)
    }

    /// Copy `source` to `target`, each given as `(bucket, object)`; `send`
    /// returns the validated destination receipt.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// let receipt = client
    ///     .copy_object(("src-bucket", "a.txt"), ("dst-bucket", "b.txt"))
    ///     .send()
    ///     .await?;
    /// assert_eq!(receipt.object_name, "b.txt");
    /// # Ok(()) }
    /// ```
    pub fn copy_object(
        &self,
        source: (impl Into<String>, impl Into<String>),
        target: (impl Into<String>, impl Into<String>),
    ) -> CopyObject {
        CopyObject {
            client: self.clone(),
            source_bucket: source.0.into(),
            source_object: source.1.into(),
            target_bucket: target.0.into(),
            target_object: target.1.into(),
            timeout: None,
        }
    }

    /// Rename `source` to `target` within one bucket.
    pub fn rename_object(
        &self,
        bucket: impl Into<String>,
        source: impl Into<String>,
        target: impl Into<String>,
    ) -> RenameObject {
        RenameObject {
            client: self.clone(),
            bucket: bucket.into(),
            source: source.into(),
            target: target.into(),
            timeout: None,
        }
    }

    /// Restore a deleted object.
    pub fn restore_object(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> RestoreObject {
        RestoreObject::new(self, bucket, object)
    }

    /// Request asynchronous CDN prefetch of an object.
    pub fn prefetch_object(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> PrefetchObject {
        PrefetchObject::new(self, bucket, object)
    }

    /// Request asynchronous CDN refresh of an object.
    pub fn refresh_object(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> RefreshObject {
        RefreshObject::new(self, bucket, object)
    }
}
