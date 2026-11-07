//! Deletion with explicit partial-failure outcomes.

use super::validate_object;
use crate::client::ClientRequest;
use crate::{Client, HttpMethod, ListObjectsRequest, Result};
use bytes::Bytes;
use serde::Deserialize;
use std::collections::HashSet;

/// One failed object from the documented bulk-delete response.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeleteObjectFailure {
    pub object_name: String,
    pub error_code: String,
    pub error_description: String,
}

/// Confirmed outcomes from completed batches. Names absent from a valid failure
/// array are considered deleted by the service contract (not independently HEAD-checked).
#[derive(Default, Clone)]
#[non_exhaustive]
pub struct DeleteObjectsOutput {
    pub deleted: Vec<String>,
    pub failures: Vec<DeleteObjectFailure>,
}

/// Incomplete deletion. `output` retains completed outcomes; `unconfirmed`
/// contains the batch for which transport was attempted but outcomes could not
/// be confirmed (including errors that cannot prove whether submission occurred).
/// `not_submitted` contains locally rejected batch names. Empty input is Ok.
/// Prefix enumeration stops at the first partial failure or request error.
/// Objects in later, unlisted pages are not represented here. Never blindly retry
/// an unconfirmed batch: the service may already have deleted those objects.
#[derive(thiserror::Error)]
#[error("bulk deletion incomplete ({deleted} confirmed deleted, {failed} failed, {unknown} unconfirmed, {local} not submitted)", deleted = .output.deleted.len(), failed = .output.failures.len(), unknown = .unconfirmed.len(), local = .not_submitted.len())]
#[non_exhaustive]
pub struct DeleteObjectsError {
    pub output: DeleteObjectsOutput,
    pub unconfirmed: Vec<String>,
    /// Batch rejected locally before submission. Never included in `unconfirmed`.
    pub not_submitted: Vec<String>,
    #[source]
    pub source: Option<Box<crate::error::FdsError>>,
}

impl std::fmt::Debug for DeleteObjectsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Debug for DeleteObjectsOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeleteObjectsOutput")
            .field("deleted_count", &self.deleted.len())
            .field("failure_count", &self.failures.len())
            .finish()
    }
}

impl std::fmt::Debug for DeleteObjectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeleteObjectFailure (details redacted)")
    }
}

impl Client {
    /// Delete an object (DELETE `/{b}/{o}`).
    pub async fn delete_object(&self, bucket: &str, object: &str) -> Result<()> {
        validate_object(bucket, object)?;
        self.do_discard(ClientRequest::new(bucket, object, HttpMethod::Delete))
            .await
    }

    /// Delete one batch. Empty input makes no request. A required JSON failure
    /// array is decoded even for HTTP 200; partial failure is Err with outcomes.
    /// Local target validation rejects the entire batch with no unconfirmed names.
    /// Duplicate input names remain on the wire and in successful output in input
    /// order. A failure for a name excludes every occurrence from `deleted`;
    /// failure receipts retain service order and multiplicity.
    pub async fn delete_objects(
        &self,
        bucket: &str,
        names: &[String],
        put_to_trash: bool,
    ) -> std::result::Result<DeleteObjectsOutput, DeleteObjectsError> {
        let mut output = DeleteObjectsOutput::default();
        if names.is_empty() {
            return Ok(output);
        }
        for name in names {
            validate_object(bucket, name).map_err(|source| DeleteObjectsError {
                output: DeleteObjectsOutput::default(),
                not_submitted: names.to_vec(),
                unconfirmed: Vec::new(),
                source: Some(Box::new(source)),
            })?;
        }
        let requested: HashSet<&str> = names.iter().map(String::as_str).collect();
        let response: Result<Vec<DeleteObjectFailure>> = async {
            let mut cr = ClientRequest::new(bucket, "", HttpMethod::Put);
            cr.query.add("enableTrash", put_to_trash.to_string());
            cr.query.add("deleteObjects", "");
            cr.body = Some(Bytes::from(serde_json::to_vec(names)?).into());
            self.do_json(cr).await
        }
        .await;
        let failures = response.map_err(|source| {
            // For this operation Invalid/Endpoint/Json and request-builder errors
            // arise before transport. Invalid response JSON is a Response error.
            let local = matches!(
                source,
                crate::FdsError::Invalid(_) | crate::FdsError::Endpoint | crate::FdsError::Json(_)
            ) || matches!(&source, crate::FdsError::Http(e) if e.is_builder());
            DeleteObjectsError {
                output: DeleteObjectsOutput::default(),
                unconfirmed: if local { Vec::new() } else { names.to_vec() },
                not_submitted: if local { names.to_vec() } else { Vec::new() },
                source: Some(Box::new(source)),
            }
        })?;
        // An unknown failure key invalidates success inference for this batch.
        if failures
            .iter()
            .any(|failure| !requested.contains(failure.object_name.as_str()))
        {
            output.failures = failures;
            return Err(DeleteObjectsError {
                output,
                not_submitted: Vec::new(),
                unconfirmed: names.to_vec(),
                source: Some(Box::new(crate::error::FdsError::invalid(
                    "bulk deletion returned an unrequested object",
                ))),
            });
        }
        let failed: HashSet<&str> = failures.iter().map(|f| f.object_name.as_str()).collect();
        output.deleted = names
            .iter()
            .filter(|name| !failed.contains(name.as_str()))
            .cloned()
            .collect();
        output.failures = failures;
        if output.failures.is_empty() {
            Ok(output)
        } else {
            Err(DeleteObjectsError {
                output,
                not_submitted: Vec::new(),
                unconfirmed: Vec::new(),
                source: None,
            })
        }
    }

    /// Enumerate and delete one page at a time, stopping on incomplete results.
    /// Later list/delete errors retain all earlier confirmed outcomes. Pagination
    /// keeps the caller's bucket/prefix, never trusts echoed response scope.
    pub async fn delete_objects_with_prefix(
        &self,
        request: &ListObjectsRequest,
        put_to_trash: bool,
    ) -> std::result::Result<DeleteObjectsOutput, DeleteObjectsError> {
        let mut output = DeleteObjectsOutput::default();
        let mut request = request.clone();
        let mut seen = HashSet::new();
        if let Some(marker) = &request.marker {
            seen.insert(marker.clone());
        }
        loop {
            let listing = match self.list_objects(&request).await {
                Ok(listing) => listing,
                Err(source) => {
                    return Err(DeleteObjectsError {
                        output,
                        not_submitted: Vec::new(),
                        unconfirmed: Vec::new(),
                        source: Some(Box::new(source)),
                    })
                }
            };
            // Validate before any destructive call for this page.
            if listing
                .object_summaries
                .iter()
                .any(|o| o.object_name.is_empty() || !o.object_name.starts_with(&request.prefix))
                || (listing.truncated
                    && (listing.next_marker.is_empty()
                        || !seen.insert(listing.next_marker.clone())))
            {
                return Err(DeleteObjectsError {
                    output,
                    not_submitted: listing
                        .object_summaries
                        .iter()
                        .map(|o| o.object_name.clone())
                        .collect(),
                    unconfirmed: Vec::new(),
                    source: Some(Box::new(crate::error::FdsError::invalid(
                        "invalid prefix deletion page or pagination marker",
                    ))),
                });
            }
            let names: Vec<String> = listing
                .object_summaries
                .iter()
                .map(|o| o.object_name.clone())
                .collect();
            match self
                .delete_objects(&request.bucket_name, &names, put_to_trash)
                .await
            {
                Ok(batch) => output.deleted.extend(batch.deleted),
                Err(mut error) => {
                    output.deleted.append(&mut error.output.deleted);
                    output.failures.append(&mut error.output.failures);
                    error.output = output;
                    return Err(error);
                }
            }
            if !listing.truncated {
                return Ok(output);
            }
            request.marker = Some(listing.next_marker);
        }
    }
}
