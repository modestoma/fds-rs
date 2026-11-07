//! Deletion with explicit partial-failure outcomes.

use super::listing::MarkerGuard;
use super::validate_object;
use super::ListObjectsRequest;
use crate::client::ClientRequest;
use crate::{Client, Dispatch, Error, ErrorKind, Result};
use bytes::Bytes;
use http::Method;
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

/// Per-object bulk deletion outcomes. Names absent from a valid failure array
/// are considered deleted by the service contract (not independently HEAD-checked).
///
/// `unconfirmed` holds names of a batch whose request may have reached the
/// service but whose outcome could not be confirmed; never blindly retry them,
/// the service may already have deleted those objects.
#[derive(Default, Clone)]
#[non_exhaustive]
pub struct DeleteObjectsOutput {
    pub deleted: Vec<String>,
    pub failures: Vec<DeleteObjectFailure>,
    pub unconfirmed: Vec<String>,
}

impl std::fmt::Debug for DeleteObjectsOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeleteObjectsOutput")
            .field("deleted_count", &self.deleted.len())
            .field("failure_count", &self.failures.len())
            .field("unconfirmed_count", &self.unconfirmed.len())
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
        self.do_discard(ClientRequest::new(bucket, object, Method::DELETE))
            .await
    }

    /// Delete one batch. Empty input makes no request and returns `Ok`.
    ///
    /// `Ok` means every name was confirmed deleted. Otherwise the error's
    /// [`partial_delete_output`](crate::Error::partial_delete_output) carries
    /// the outcomes, classified by the request's [`Dispatch`]:
    ///
    /// - local target validation or a failure before sending: the original
    ///   error with `Dispatch::NotSent`; nothing was submitted and no partial
    ///   output is attached;
    /// - any later failure (error status, broken or invalid response, a failure
    ///   for an unrequested name): [`ErrorKind::OutcomeUnknown`], with every
    ///   name in `unconfirmed`;
    /// - a valid response listing per-object failures:
    ///   [`ErrorKind::PartialFailure`], with `deleted` and `failures`.
    ///
    /// Duplicate input names remain on the wire and in successful output in input
    /// order. A failure for a name excludes every occurrence from `deleted`;
    /// failure receipts retain service order and multiplicity.
    pub async fn delete_objects(
        &self,
        bucket: &str,
        names: &[String],
        put_to_trash: bool,
    ) -> Result<DeleteObjectsOutput> {
        let mut output = DeleteObjectsOutput::default();
        if names.is_empty() {
            return Ok(output);
        }
        for name in names {
            validate_object(bucket, name)?;
        }
        let requested: HashSet<&str> = names.iter().map(String::as_str).collect();
        let body = serde_json::to_vec(names).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(bucket, "", Method::PUT);
        cr.query.add("enableTrash", put_to_trash.to_string());
        cr.query.add("deleteObjects", "");
        cr.body = Some(Bytes::from(body).into());
        let failures: Vec<DeleteObjectFailure> = match self.do_json(cr).await {
            Ok(failures) => failures,
            Err(error) if error.dispatch() == Dispatch::NotSent => return Err(error),
            Err(error) => {
                output.unconfirmed = names.to_vec();
                return Err(error
                    .into_outcome_unknown(UNKNOWN)
                    .with_delete_output(output));
            }
        };
        // An unknown failure key invalidates success inference for this batch.
        if failures
            .iter()
            .any(|failure| !requested.contains(failure.object_name.as_str()))
        {
            output.failures = failures;
            output.unconfirmed = names.to_vec();
            return Err(
                Error::protocol("bulk deletion returned an unrequested object")
                    .into_outcome_unknown(UNKNOWN)
                    .with_delete_output(output),
            );
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
            Err(Error::new(
                ErrorKind::PartialFailure,
                "bulk deletion failed for some objects",
            )
            .with_dispatch(Dispatch::Sent)
            .with_context("deleted", output.deleted.len())
            .with_context("failed", output.failures.len())
            .with_delete_output(output))
        }
    }

    /// Enumerate and delete one page at a time, stopping on incomplete results.
    ///
    /// Any error carries every outcome confirmed so far (including earlier
    /// pages) in [`partial_delete_output`](crate::Error::partial_delete_output).
    /// A listing failure keeps its own kind; an invalid page or pagination
    /// marker is [`ErrorKind::ProtocolViolation`] and nothing from that page was
    /// submitted; batch failures are classified as in
    /// [`delete_objects`](Self::delete_objects). The error's dispatch describes
    /// the failing request only. Objects in later, unlisted pages are not
    /// represented. Pagination keeps the caller's bucket/prefix, never trusts
    /// echoed response scope. Any repeated pagination marker, including the
    /// caller's, is rejected before that page is submitted.
    pub async fn delete_objects_with_prefix(
        &self,
        request: &ListObjectsRequest,
        put_to_trash: bool,
    ) -> Result<DeleteObjectsOutput> {
        let mut output = DeleteObjectsOutput::default();
        let mut request = request.clone();
        let mut guard = MarkerGuard::exact(request.marker.as_deref());
        loop {
            let listing = match self.list_objects(&request).await {
                Ok(listing) => listing,
                Err(error) => return Err(error.with_delete_output(output)),
            };
            // Validate before any destructive call for this page.
            if listing
                .object_summaries
                .iter()
                .any(|o| o.object_name.is_empty() || !o.object_name.starts_with(&request.prefix))
                || (listing.truncated && !guard.advance(&listing.next_marker))
            {
                return Err(
                    Error::protocol("invalid prefix deletion page or pagination marker")
                        .with_delete_output(output),
                );
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
                    if let Some(mut batch) = error.take_delete_output() {
                        output.deleted.append(&mut batch.deleted);
                        output.failures.append(&mut batch.failures);
                        output.unconfirmed.append(&mut batch.unconfirmed);
                    }
                    return Err(error.with_delete_output(output));
                }
            }
            if !listing.truncated {
                return Ok(output);
            }
            request.marker = Some(listing.next_marker);
        }
    }
}

const UNKNOWN: &str =
    "bulk deletion outcome unknown; the objects may already be deleted, do not retry blindly";
