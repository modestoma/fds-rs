//! Object listing models and lazy pagination.

use crate::acl::Owner;
use crate::client::ClientRequest;
use crate::constant::DEFAULT_LIST_OBJECTS_MAX_KEYS;
use crate::{Client, Error, HttpMethod, Result};
use chrono::{DateTime, Utc};
use futures_util::{stream, Stream, TryStreamExt};
use serde::Deserialize;

/// One entry in a listing (Go `ObjectSummary`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObjectSummary {
    #[serde(rename = "etag", default, deserialize_with = "crate::de::null_default")]
    pub etag: String,
    #[serde(rename = "name", default, deserialize_with = "crate::de::null_default")]
    pub object_name: String,
    #[serde(
        rename = "owner",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub owner: Owner,
    #[serde(rename = "size", default, deserialize_with = "crate::de::null_default")]
    pub size: i64,
    #[serde(
        rename = "lastModified",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub last_modified: Option<DateTime<Utc>>,
    #[serde(
        rename = "uploadTime",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub upload_time: i64,
}

/// Input of `list_objects` (Go `ListObjectsRequest`). No omitempty: all three
/// query params are always emitted.
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

/// Result of `list_objects` (Go `ObjectListing`); also carries the state needed
/// to fetch the next batch.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObjectListing {
    #[serde(rename = "name", default, deserialize_with = "crate::de::null_default")]
    pub bucket_name: String,
    #[serde(
        rename = "prefix",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub prefix: String,
    #[serde(
        rename = "maxKeys",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub max_keys: i32,
    #[serde(
        rename = "marker",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub marker: String,
    #[serde(
        rename = "truncated",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub truncated: bool,
    #[serde(
        rename = "nextMarker",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub next_marker: String,
    #[serde(
        rename = "delimiter",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub delimiter: String,
    #[serde(
        rename = "objects",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub object_summaries: Vec<ObjectSummary>,
    #[serde(
        rename = "commonPrefixes",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub common_prefixes: Vec<String>,
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
        let checkpoint = self.marker.clone();
        stream::try_unfold(
            (self, false, checkpoint, 1usize, 0usize),
            move |(mut options, done, mut checkpoint, mut power, mut distance)| async move {
                if done {
                    return Ok(None);
                }
                let page = client.list_objects(&options).await?;
                if page.truncated {
                    let next = &page.next_marker;
                    if next.is_empty()
                        || options.marker.as_ref() == Some(next)
                        || checkpoint.as_ref() == Some(next)
                    {
                        return Err(Error::protocol("listing has missing or cyclic nextMarker"));
                    }
                    distance += 1;
                    if distance == power {
                        checkpoint = Some(next.clone());
                        power = power.saturating_mul(2);
                        distance = 0;
                    }
                    options.marker = Some(next.clone());
                }
                let done = !page.truncated;
                Ok(Some((page, (options, done, checkpoint, power, distance))))
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

impl Client {
    /// List objects (GET `/{b}?prefix&delimiter&maxKeys`).
    pub async fn list_objects(&self, req: &ListObjectsRequest) -> Result<ObjectListing> {
        crate::bucket::validate_bucket(&req.bucket_name)?;
        if req.max_keys <= 0 {
            return Err(Error::invalid_input("max_keys must be positive"));
        }
        let mut cr = ClientRequest::new(&req.bucket_name, "", HttpMethod::Get).read_only();
        if let Some(marker) = &req.marker {
            cr.query.add("marker", marker);
        }
        cr.query.add("prefix", &req.prefix);
        cr.query.add("delimiter", &req.delimiter);
        cr.query.add("maxKeys", req.max_keys.to_string());
        self.do_json(cr).await
    }
}
