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

crate::de::lenient! {
    /// One entry in a listing.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct ObjectSummary {
        pub etag: String,
        #[serde(rename = "name")]
        pub object_name: String,
        pub owner: Owner,
        pub size: i64,
        pub last_modified: Option<DateTime<Utc>>,
        pub upload_time: i64,
    }
}

/// Input of `list_objects`. `prefix`, `delimiter` and `maxKeys` are always
/// emitted, even when empty.
#[derive(Debug, Clone)]
pub struct ListObjectsRequest {
    pub bucket_name: String,
    pub prefix: String,
    pub delimiter: String,
    pub max_keys: i32,
    pub marker: Option<String>,
}

impl Default for ListObjectsRequest {
    fn default() -> Self {
        Self {
            bucket_name: String::new(),
            prefix: String::new(),
            delimiter: String::new(),
            max_keys: DEFAULT_LIST_OBJECTS_MAX_KEYS,
            marker: None,
        }
    }
}

crate::de::lenient! {
    /// Result of `list_objects`; also carries the state needed to fetch the
    /// next batch.
    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct ObjectListing {
        #[serde(rename = "name")]
        pub bucket_name: String,
        pub prefix: String,
        pub max_keys: i32,
        pub marker: String,
        pub truncated: bool,
        pub next_marker: String,
        pub delimiter: String,
        #[serde(rename = "objects")]
        pub object_summaries: Vec<ObjectSummary>,
        pub common_prefixes: Vec<String>,
    }
}

impl ListObjectsRequest {
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket_name: bucket.into(),
            ..Self::default()
        }
    }
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }
    pub fn delimiter(mut self, delimiter: impl Into<String>) -> Self {
        self.delimiter = delimiter.into();
        self
    }
    pub fn max_keys(mut self, max_keys: i32) -> Self {
        self.max_keys = max_keys;
        self
    }
    pub async fn send(self, client: &Client) -> Result<ObjectListing> {
        client.list_objects(&self).await
    }

    pub fn marker(mut self, marker: impl Into<String>) -> Self {
        self.marker = Some(marker.into());
        self
    }
    /// Yield pages on demand, including empty truncated pages. A service/transport
    /// error or invalid marker yields one error, then ends the stream.
    ///
    /// Markers are opaque. Brent cycle detection retains two markers instead of
    /// an ever-growing set. Adjacent repetition is rejected immediately; longer
    /// cycles are detected eventually and may yield repeated pages before detection.
    /// This is not a snapshot or an object deduplication guarantee.
    pub fn pages(self, client: &Client) -> impl Stream<Item = Result<ObjectListing>> + '_ {
        let guard = MarkerGuard::new(self.marker.as_deref());
        stream::try_unfold(
            (self, false, guard),
            move |(mut options, done, mut guard)| async move {
                if done {
                    return Ok(None);
                }
                let page = client.list_objects(&options).await?;
                if page.truncated {
                    if !guard.advance(&page.next_marker) {
                        return Err(Error::protocol("listing has missing or cyclic nextMarker"));
                    }
                    options.marker = Some(page.next_marker.clone());
                }
                let done = !page.truncated;
                Ok(Some((page, (options, done, guard))))
            },
        )
    }
    /// Flatten pages without prefetching or retaining previous pages. Skips empty
    /// pages and common prefixes; use pages() when directory prefixes are needed.
    pub fn objects(self, client: &Client) -> impl Stream<Item = Result<ObjectSummary>> + '_ {
        self.pages(client)
            .map_ok(|page| stream::iter(page.object_summaries.into_iter().map(Ok)))
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
    /// List objects (GET `/{b}?prefix&delimiter&maxKeys`).
    pub async fn list_objects(&self, req: &ListObjectsRequest) -> Result<ObjectListing> {
        crate::bucket::validate_bucket(&req.bucket_name)?;
        if req.max_keys <= 0 {
            return Err(Error::invalid_input("max_keys must be positive"));
        }
        let mut cr = ClientRequest::new(&req.bucket_name, "", Method::GET).read_only();
        if let Some(marker) = &req.marker {
            cr.query.add("marker", marker);
        }
        cr.query.add("prefix", &req.prefix);
        cr.query.add("delimiter", &req.delimiter);
        cr.query.add("maxKeys", req.max_keys.to_string());
        self.do_json(cr).await
    }
}
