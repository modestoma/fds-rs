//! Object listing models and lazy pagination.

use crate::acl::Owner;
use crate::client::ClientRequest;
use crate::constant::DEFAULT_LIST_OBJECTS_MAX_KEYS;
use crate::{Client, Error, Result};
use chrono::{DateTime, Utc};
use futures_util::{stream, Stream, TryStreamExt};
use http::Method;
use serde::Deserialize;
use std::collections::HashSet;
use std::num::NonZeroU32;
use std::time::Duration;

crate::de::lenient! {
    /// One entry in a listing. Missing or `null` keys read as empty, zero or
    /// `None`.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct ObjectSummary {
        pub etag: String,
        #[serde(rename = "name")]
        pub object_name: String,
        pub owner: Owner,
        /// Size in bytes; a negative value is a protocol violation.
        pub size: u64,
        /// An RFC 3339 `lastModified` string, when the service sends one.
        pub last_modified: Option<DateTime<Utc>>,
        /// Upload time, decoded from Unix epoch milliseconds; a negative or
        /// unrepresentable timestamp is a protocol violation.
        pub upload_time: Option<DateTime<Utc>> => "crate::de::opt_millis",
    }
}

crate::de::lenient! {
    /// One page of [`Client::list_objects`]; also carries the state needed to fetch the
    /// next batch. Missing or `null` keys read as empty, zero or `false`.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct ObjectListing {
        #[serde(rename = "name")]
        pub bucket_name: String,
        pub prefix: String,
        /// The page limit the service applied, as echoed in `maxKeys`.
        pub max_keys: u32,
        pub marker: String,
        pub truncated: bool,
        pub next_marker: String,
        pub delimiter: String,
        pub objects: Vec<ObjectSummary>,
        pub common_prefixes: Vec<String>,
    }
}

/// Builder for [`Client::list_objects`].
///
/// `prefix`, `delimiter` and `maxKeys` are always sent, even when empty;
/// `marker` is sent whenever it was set, even to an empty string.
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited or a stream is polled"]
pub struct ListObjects {
    client: Client,
    bucket: String,
    prefix: String,
    delimiter: String,
    max_keys: NonZeroU32,
    marker: Option<String>,
    timeout: Option<Duration>,
    /// Use [`MarkerGuard::exact`] instead of [`MarkerGuard::new`].
    exact_markers: bool,
}

impl ListObjects {
    pub(crate) fn new(client: &Client, bucket: impl Into<String>) -> Self {
        Self {
            client: client.clone(),
            bucket: bucket.into(),
            prefix: String::new(),
            delimiter: String::new(),
            max_keys: DEFAULT_LIST_OBJECTS_MAX_KEYS,
            marker: None,
            timeout: None,
            exact_markers: false,
        }
    }

    /// Only list names starting with `prefix`. Default: empty (everything).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Group names that contain `delimiter` after the prefix into
    /// [`ObjectListing::common_prefixes`] instead of listing them. Default:
    /// empty (no grouping).
    pub fn delimiter(mut self, delimiter: impl Into<String>) -> Self {
        self.delimiter = delimiter.into();
        self
    }

    /// The most entries one page may hold. Default: 1000.
    pub fn max_keys(mut self, max_keys: NonZeroU32) -> Self {
        self.max_keys = max_keys;
        self
    }

    /// Start after this opaque marker, usually a previous page's
    /// [`ObjectListing::next_marker`].
    pub fn marker(mut self, marker: impl Into<String>) -> Self {
        self.marker = Some(marker.into());
        self
    }

    timeout_option!();

    /// Make [`into_pages`](Self::into_pages) reject every repeated marker
    /// before yielding that page, at the cost of remembering all markers.
    pub(crate) fn exact_markers(mut self) -> Self {
        self.exact_markers = true;
        self
    }

    /// GET `/{bucket}?prefix&delimiter&maxKeys[&marker]`: one page.
    ///
    /// Check [`ObjectListing::truncated`] and continue from
    /// [`ObjectListing::next_marker`], or use [`into_pages`](Self::into_pages).
    pub async fn send(self) -> Result<ObjectListing> {
        self.fetch().await
    }

    async fn fetch(&self) -> Result<ObjectListing> {
        crate::bucket::validate_bucket(&self.bucket)?;
        let mut cr = ClientRequest::new(&self.bucket, "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        if let Some(marker) = &self.marker {
            cr.query.add("marker", marker);
        }
        cr.query.add("prefix", &self.prefix);
        cr.query.add("delimiter", &self.delimiter);
        cr.query.add("maxKeys", self.max_keys.to_string());
        self.client.do_json(cr).await
    }

    /// Fetch pages on demand, starting at the marker (if any) and following
    /// `nextMarker` while the listing is truncated. Empty truncated pages are
    /// yielded too. Nothing is prefetched; the timeout applies to each page
    /// request separately.
    ///
    /// Local validation, service, transport and pagination failures are
    /// yielded as one error, after which the stream ends. A truncated page
    /// without a `nextMarker`, or with a marker seen before, is a
    /// [`ErrorKind::ProtocolViolation`](crate::ErrorKind::ProtocolViolation)
    /// instead of that page.
    ///
    /// Markers are opaque. Brent cycle detection retains two markers instead
    /// of an ever-growing set: an adjacent repetition is rejected immediately,
    /// a longer cycle eventually, possibly after repeated pages. This is not a
    /// snapshot or an object deduplication guarantee.
    pub fn into_pages(self) -> impl Stream<Item = Result<ObjectListing>> + Send + 'static {
        let start = self.marker.as_deref();
        let guard = if self.exact_markers {
            MarkerGuard::exact(start)
        } else {
            MarkerGuard::new(start)
        };
        stream::try_unfold(
            (self, false, guard),
            |(mut list, done, mut guard)| async move {
                if done {
                    return Ok(None);
                }
                let page = list.fetch().await?;
                if page.truncated {
                    if !guard.advance(&page.next_marker) {
                        return Err(Error::protocol("listing has missing or cyclic nextMarker"));
                    }
                    list.marker = Some(page.next_marker.clone());
                }
                let done = !page.truncated;
                Ok(Some((page, (list, done, guard))))
            },
        )
    }

    /// The entries of [`into_pages`](Self::into_pages), flattened, with the
    /// same laziness and error behavior. Common prefixes are skipped; use
    /// `into_pages` when directory prefixes are needed.
    pub fn into_objects(self) -> impl Stream<Item = Result<ObjectSummary>> + Send + 'static {
        self.into_pages()
            .map_ok(|page| stream::iter(page.objects.into_iter().map(Ok)))
            .try_flatten()
    }
}

/// Rejects missing or cyclic `nextMarker`s across one pagination run.
///
/// Markers are opaque. By default Brent's cycle detection retains two markers
/// instead of an ever-growing set: a repetition of the previous marker or of
/// the latest checkpoint is rejected at once, a longer cycle within a bounded
/// number of further pages, so a cycle may yield repeated pages first.
/// [`exact`](Self::exact) additionally remembers every marker and rejects any
/// repetition at once, for destructive callers.
pub(crate) struct MarkerGuard {
    previous: Option<String>,
    checkpoint: Option<String>,
    power: usize,
    distance: usize,
    seen: Option<HashSet<String>>,
}

impl MarkerGuard {
    /// Bounded memory; start from the caller's marker, if any.
    pub(crate) fn new(start: Option<&str>) -> Self {
        Self {
            previous: start.map(str::to_owned),
            checkpoint: start.map(str::to_owned),
            power: 1,
            distance: 0,
            seen: None,
        }
    }

    /// Reject every repetition immediately, retaining all markers.
    pub(crate) fn exact(start: Option<&str>) -> Self {
        Self {
            seen: Some(start.map(str::to_owned).into_iter().collect()),
            ..Self::new(start)
        }
    }

    /// Accept `next` as the marker of the following page; `false` when it is
    /// empty or repeats an earlier marker.
    pub(crate) fn advance(&mut self, next: &str) -> bool {
        if next.is_empty()
            || self.previous.as_deref() == Some(next)
            || self.checkpoint.as_deref() == Some(next)
            || self
                .seen
                .as_mut()
                .is_some_and(|seen| !seen.insert(next.to_owned()))
        {
            return false;
        }
        self.distance += 1;
        if self.distance == self.power {
            self.checkpoint = Some(next.to_owned());
            self.power = self.power.saturating_mul(2);
            self.distance = 0;
        }
        self.previous = Some(next.to_owned());
        true
    }
}

impl Client {
    /// List the objects of a bucket: one page with
    /// [`send`](ListObjects::send), or every page lazily with
    /// [`into_pages`](ListObjects::into_pages) /
    /// [`into_objects`](ListObjects::into_objects).
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// use futures_util::TryStreamExt;
    /// use std::num::NonZeroU32;
    ///
    /// let objects = client
    ///     .list_objects("bucket")
    ///     .prefix("logs/")
    ///     .max_keys(NonZeroU32::new(100).unwrap())
    ///     .into_objects();
    /// futures_util::pin_mut!(objects);
    /// while let Some(object) = objects.try_next().await? {
    ///     println!("{} {}", object.object_name, object.size);
    /// }
    /// # Ok(()) }
    /// ```
    pub fn list_objects(&self, bucket: impl Into<String>) -> ListObjects {
        ListObjects::new(self, bucket)
    }
}
