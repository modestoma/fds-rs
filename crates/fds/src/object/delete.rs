//! Single and bulk deletion. Per-object failures of a batch are part of the
//! `Ok` output; see [`DeleteObjects::send`] and [`DeletePrefix::send`].

use super::validate_object;
use crate::client::ClientRequest;
use crate::constant::DEFAULT_LIST_OBJECTS_MAX_KEYS;
use crate::{Client, Dispatch, Error, ErrorKind, Result};
use bytes::Bytes;
use futures_util::TryStreamExt;
use http::Method;
use serde::Deserialize;
use std::collections::HashSet;
use std::num::NonZeroU32;
use std::time::Duration;

/// One failed object from the documented bulk-delete response.
///
/// `Debug` shows the service's `error_code` (when it looks like a plain
/// identifier) but not the object name or the description: object names are
/// user data that often identify people or files, and the description is
/// free service text that can echo the name. Read the fields directly when
/// they are needed.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeleteObjectFailure {
    pub object_name: String,
    pub error_code: String,
    pub error_description: String,
}

impl std::fmt::Debug for DeleteObjectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("DeleteObjectFailure");
        match crate::error::printable_code(&self.error_code) {
            Some(code) => s.field("error_code", &code),
            None => s.field("error_code", &format_args!("<withheld>")),
        };
        s.finish_non_exhaustive()
    }
}

/// Per-object outcomes of bulk deletion.
///
/// Names absent from a valid failure array are considered deleted by the
/// service contract (not independently HEAD-checked).
///
/// `unconfirmed` holds names whose request may have reached the service but
/// whose outcome could not be confirmed; never blindly retry them, the
/// service may already have deleted those objects. It is only filled in the
/// output attached to an error
/// ([`Error::partial_delete_output`](crate::Error::partial_delete_output));
/// it is always empty in an `Ok` value.
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

object_builder! {
    /// Builder for [`Client::delete_object`].
    DeleteObject
}

impl DeleteObject {
    /// DELETE `/{bucket}/{object}`.
    pub async fn send(self) -> Result<()> {
        validate_object(&self.bucket, &self.object)?;
        let cr = ClientRequest::new(&self.bucket, &self.object, Method::DELETE)
            .with_timeout(self.timeout)?;
        self.client.do_discard(cr).await
    }
}

/// Builder for [`Client::delete_objects`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct DeleteObjects {
    client: Client,
    bucket: String,
    keys: Vec<String>,
    move_to_trash: bool,
    timeout: Option<Duration>,
}

impl DeleteObjects {
    /// Add object names to the batch, after any added before. Duplicates are
    /// kept and sent as given.
    pub fn keys<I>(mut self, keys: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.keys.extend(keys.into_iter().map(Into::into));
        self
    }

    /// Move the objects to the bucket's trash instead of deleting them
    /// permanently (`enableTrash=true`). Default: permanent deletion.
    pub fn move_to_trash(mut self) -> Self {
        self.move_to_trash = true;
        self
    }

    timeout_option!();

    /// PUT `/{bucket}?deleteObjects&enableTrash` with the names as one JSON
    /// array. The batch is not split to fit service limits.
    ///
    /// An empty batch makes no request and returns an empty `Ok`, without
    /// validating anything.
    ///
    /// - `Ok`: the service answered with a valid failure array. Every name it
    ///   lists is in `failures` (service order and multiplicity), every other
    ///   name in `deleted` (input order, duplicates kept). Per-object failures
    ///   are a normal result: check `failures`. `unconfirmed` is empty.
    /// - Local validation (bucket and object names, the timeout) or a failure
    ///   before sending: the original error, with [`Dispatch::NotSent`];
    ///   nothing was submitted and no output is attached.
    /// - Any failure after the request may have been sent (an error status, a
    ///   broken or invalid response, a failure for a name that was not
    ///   requested): [`ErrorKind::OutcomeUnknown`], with the cause as its
    ///   source and every name in the `unconfirmed` list of
    ///   [`partial_delete_output`](crate::Error::partial_delete_output).
    pub async fn send(self) -> Result<DeleteObjectsOutput> {
        let mut output = DeleteObjectsOutput::default();
        if self.keys.is_empty() {
            return Ok(output);
        }
        for name in &self.keys {
            validate_object(&self.bucket, name)?;
        }
        let body = serde_json::to_vec(&self.keys).map_err(Error::json_encode)?;
        let mut cr =
            ClientRequest::new(&self.bucket, "", Method::PUT).with_timeout(self.timeout)?;
        cr.query.add("enableTrash", self.move_to_trash.to_string());
        cr.query.add("deleteObjects", "");
        cr.body = Some(Bytes::from(body).into());
        let failures: Vec<DeleteObjectFailure> = match self.client.do_json(cr).await {
            Ok(failures) => failures,
            Err(error) if error.dispatch() == Dispatch::NotSent => return Err(error),
            Err(error) => {
                output.unconfirmed = self.keys;
                return Err(error
                    .into_outcome_unknown(UNKNOWN)
                    .with_delete_output(output));
            }
        };
        let requested: HashSet<&str> = self.keys.iter().map(String::as_str).collect();
        // An unknown failure key invalidates success inference for this batch.
        if failures
            .iter()
            .any(|failure| !requested.contains(failure.object_name.as_str()))
        {
            output.failures = failures;
            output.unconfirmed = self.keys;
            return Err(
                Error::protocol("bulk deletion returned an unrequested object")
                    .into_outcome_unknown(UNKNOWN)
                    .with_delete_output(output),
            );
        }
        let failed: HashSet<&str> = failures.iter().map(|f| f.object_name.as_str()).collect();
        output.deleted = self
            .keys
            .iter()
            .filter(|name| !failed.contains(name.as_str()))
            .cloned()
            .collect();
        output.failures = failures;
        Ok(output)
    }
}

/// Builder for [`Client::delete_prefix`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct DeletePrefix {
    client: Client,
    bucket: String,
    prefix: String,
    marker: Option<String>,
    delimiter: String,
    page_size: NonZeroU32,
    move_to_trash: bool,
    timeout: Option<Duration>,
}

impl DeletePrefix {
    /// Start after `marker` instead of at the beginning of the prefix: the
    /// first listing sends it as `marker` (even when it is empty), so names
    /// up to and including the marker are not listed and not deleted. Use a
    /// `nextMarker` of an earlier listing, for example to resume a run.
    ///
    /// The marker counts as already seen by the page check (see
    /// [`send`](Self::send)): a later page whose `nextMarker` returns to it
    /// stops the run before that page is deleted.
    ///
    /// Default: no `marker` parameter; the run starts at the beginning.
    pub fn marker(mut self, marker: impl Into<String>) -> Self {
        self.marker = Some(marker.into());
        self
    }

    /// Only delete the current level below the prefix: the listing sends
    /// `delimiter`, so the service groups names that contain it after the
    /// prefix into common prefixes, which are neither deleted nor descended
    /// into. Names the service lists as objects are deleted; its grouping is
    /// not checked again.
    ///
    /// Default: empty (no grouping), so names below nested "directories" are
    /// deleted too. An empty delimiter is the same as not calling this.
    pub fn delimiter(mut self, delimiter: impl Into<String>) -> Self {
        self.delimiter = delimiter.into();
        self
    }

    /// How many names to list, and so delete, per page (`maxKeys`).
    /// Default: 1000.
    pub fn page_size(mut self, page_size: NonZeroU32) -> Self {
        self.page_size = page_size;
        self
    }

    /// Move the objects to the bucket's trash instead of deleting them
    /// permanently. Default: permanent deletion.
    pub fn move_to_trash(mut self) -> Self {
        self.move_to_trash = true;
        self
    }

    timeout_option!();

    /// List the prefix page by page (with
    /// [`ListObjects::into_pages`](super::ListObjects::into_pages)) and delete
    /// each page as one [`DeleteObjects`] batch before listing the next.
    /// The timeout applies to each list and delete request separately.
    ///
    /// The bucket and the prefix select objects, narrowed only by the
    /// explicit [`marker`](Self::marker) (start position) and
    /// [`delimiter`](Self::delimiter) (current level only). Without them
    /// every name below the prefix is deleted, nested "directories"
    /// included. Pagination keeps the caller's bucket, prefix and delimiter,
    /// never the scope echoed by the service.
    ///
    /// Before a page is submitted it is checked: every name must be non-empty
    /// and start with the prefix, and a truncated page's `nextMarker` must be
    /// present and differ from every marker seen in this run, the starting
    /// marker included (all are remembered, so a cycle of any length stops
    /// before a page is deleted twice). A failed check is [`ErrorKind::ProtocolViolation`] and nothing
    /// from that page was submitted.
    ///
    /// `Ok` means every listed object was confirmed deleted; `failures` and
    /// `unconfirmed` are empty. Otherwise the run stops at the first problem
    /// and the error carries every outcome confirmed so far, earlier pages
    /// included, in [`partial_delete_output`](crate::Error::partial_delete_output):
    ///
    /// - a batch reporting per-object failures: [`ErrorKind::PartialFailure`]
    ///   (later pages are not listed), with that batch's `deleted` and
    ///   `failures` added;
    /// - a batch whose outcome is unknown: [`ErrorKind::OutcomeUnknown`], with
    ///   that batch's names in `unconfirmed`;
    /// - a listing failure or a failed page check: that error, with nothing
    ///   from the page added.
    ///
    /// The error's dispatch describes the failing request only. Objects in
    /// pages that were never listed are not represented.
    pub async fn send(self) -> Result<DeleteObjectsOutput> {
        let mut output = DeleteObjectsOutput::default();
        let pages = self
            .client
            .list_objects(&self.bucket)
            .prefix(&self.prefix)
            .delimiter(&self.delimiter)
            .max_keys(self.page_size)
            .exact_markers();
        let pages = match self.marker {
            Some(marker) => pages.marker(marker),
            None => pages,
        };
        let pages = match self.timeout {
            Some(timeout) => pages.timeout(timeout),
            None => pages,
        };
        let pages = pages.into_pages();
        futures_util::pin_mut!(pages);
        loop {
            let page = match pages.try_next().await {
                Ok(Some(page)) => page,
                Ok(None) => return Ok(output),
                Err(error) => return Err(error.with_delete_output(output)),
            };
            if page
                .objects
                .iter()
                .any(|o| o.object_name.is_empty() || !o.object_name.starts_with(&self.prefix))
            {
                return Err(Error::protocol(
                    "prefix deletion page lists a name outside the prefix",
                )
                .with_delete_output(output));
            }
            let mut batch = self
                .client
                .delete_objects(&self.bucket)
                .keys(page.objects.into_iter().map(|object| object.object_name));
            if self.move_to_trash {
                batch = batch.move_to_trash();
            }
            if let Some(timeout) = self.timeout {
                batch = batch.timeout(timeout);
            }
            match batch.send().await {
                Ok(mut batch) if batch.failures.is_empty() => {
                    output.deleted.append(&mut batch.deleted)
                }
                Ok(mut batch) => {
                    let error = Error::new(
                        ErrorKind::PartialFailure,
                        "bulk deletion failed for some objects",
                    )
                    .with_dispatch(Dispatch::Sent)
                    .with_context("deleted", batch.deleted.len())
                    .with_context("failed", batch.failures.len());
                    output.deleted.append(&mut batch.deleted);
                    output.failures.append(&mut batch.failures);
                    return Err(error.with_delete_output(output));
                }
                Err(mut error) => {
                    if let Some(mut batch) = error.take_delete_output() {
                        output.deleted.append(&mut batch.deleted);
                        output.failures.append(&mut batch.failures);
                        output.unconfirmed.append(&mut batch.unconfirmed);
                    }
                    return Err(error.with_delete_output(output));
                }
            }
        }
    }
}

impl Client {
    /// Delete one object.
    pub fn delete_object(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> DeleteObject {
        DeleteObject::new(self, bucket, object)
    }

    /// Delete a batch of objects in one request; per-object failures are
    /// returned in `Ok`. See [`DeleteObjects::send`].
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// let output = client
    ///     .delete_objects("bucket")
    ///     .keys(["a.txt", "b.txt"])
    ///     .move_to_trash()
    ///     .send()
    ///     .await?;
    /// for failure in &output.failures {
    ///     eprintln!("{}: {}", failure.object_name, failure.error_code);
    /// }
    /// # Ok(()) }
    /// ```
    pub fn delete_objects(&self, bucket: impl Into<String>) -> DeleteObjects {
        DeleteObjects {
            client: self.clone(),
            bucket: bucket.into(),
            keys: Vec::new(),
            move_to_trash: false,
            timeout: None,
        }
    }

    /// Delete every object whose name starts with `prefix`, one listed page
    /// per batch. See [`DeletePrefix::send`]; an empty prefix means the whole
    /// bucket. [`DeletePrefix::marker`] and [`DeletePrefix::delimiter`]
    /// narrow the run to a start position and the current level.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// // Delete logs/2026-11-07.txt but keep logs/archive/old.txt.
    /// let output = client
    ///     .delete_prefix("bucket", "logs/")
    ///     .delimiter("/")
    ///     .send()
    ///     .await?;
    /// println!("{} deleted", output.deleted.len());
    /// # Ok(()) }
    /// ```
    pub fn delete_prefix(
        &self,
        bucket: impl Into<String>,
        prefix: impl Into<String>,
    ) -> DeletePrefix {
        DeletePrefix {
            client: self.clone(),
            bucket: bucket.into(),
            prefix: prefix.into(),
            marker: None,
            delimiter: String::new(),
            page_size: DEFAULT_LIST_OBJECTS_MAX_KEYS,
            move_to_trash: false,
            timeout: None,
        }
    }
}

const UNKNOWN: &str =
    "bulk deletion outcome unknown; the objects may already be deleted, do not retry blindly";
