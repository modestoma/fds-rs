//! Object metadata: the typed system attributes an object reports, the user
//! metadata, and their crate-internal wire forms.
//!
//! The three wire forms are asymmetric and never public:
//! 1. **send**: request headers (put, multipart, presign) or the
//!    `set_object_metadata` body `{"rawMeta": {k:v,...}}` (map);
//! 2. **basic info**: `ObjectBasicInfo.metadataBean`
//!    = `{"rawMeta":{"entry":[{key,value},...]}}` (entry-list);
//! 3. **response headers**: HEAD, GET and GET `?metadata`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderName, ETAG};
use serde::{Deserialize, Deserializer, Serialize};

use crate::constant::{
    HTTP_HEADER_AUTHORIZATION, HTTP_HEADER_CACHE_CONTROL, HTTP_HEADER_CONTENT_ENCODING,
    HTTP_HEADER_CONTENT_LENGTH, HTTP_HEADER_CONTENT_MD5, HTTP_HEADER_CONTENT_METADATA_LENGTH,
    HTTP_HEADER_CONTENT_RANGE, HTTP_HEADER_CONTENT_TYPE, HTTP_HEADER_CRC64ECMA, HTTP_HEADER_DATE,
    HTTP_HEADER_LAST_CHECKED, HTTP_HEADER_LAST_MODIFIED, HTTP_HEADER_MULTIPART_UPLOAD_MODE,
    HTTP_HEADER_ONGOING_RESTORE, HTTP_HEADER_RANGE, HTTP_HEADER_RESTORE_EXPIRE_DATE,
    HTTP_HEADER_SERVER_SIDE_ENCRYPTION, HTTP_HEADER_STORAGE_CLASS, HTTP_HEADER_UPLOAD_TIME,
    XIAOMI_META_PREFIX,
};
use crate::error::{Error, Result};
use crate::object::StorageClass;

/// The protocol headers a response-header read keeps in
/// [`ObjectMetadata::entries`], besides every `x-xiaomi-meta-*` header (all
/// lowercase).
const PREDEFINED: &[&str] = &[
    HTTP_HEADER_CACHE_CONTROL,
    HTTP_HEADER_CONTENT_LENGTH,
    HTTP_HEADER_CONTENT_ENCODING,
    HTTP_HEADER_LAST_MODIFIED,
    HTTP_HEADER_CONTENT_MD5,
    HTTP_HEADER_CONTENT_TYPE,
    HTTP_HEADER_LAST_CHECKED,
    HTTP_HEADER_UPLOAD_TIME,
    HTTP_HEADER_DATE,
    HTTP_HEADER_AUTHORIZATION,
    HTTP_HEADER_RANGE,
    HTTP_HEADER_CONTENT_RANGE,
    HTTP_HEADER_CONTENT_METADATA_LENGTH,
    HTTP_HEADER_SERVER_SIDE_ENCRYPTION,
    HTTP_HEADER_STORAGE_CLASS,
    HTTP_HEADER_ONGOING_RESTORE,
    HTTP_HEADER_RESTORE_EXPIRE_DATE,
    HTTP_HEADER_CRC64ECMA,
    HTTP_HEADER_MULTIPART_UPLOAD_MODE,
];

/// `x-xiaomi-meta-*` keys that carry system attributes, not user metadata.
const SYSTEM_META_KEYS: &[&str] = &[
    HTTP_HEADER_CONTENT_METADATA_LENGTH,
    HTTP_HEADER_SERVER_SIDE_ENCRYPTION,
    HTTP_HEADER_STORAGE_CLASS,
    HTTP_HEADER_ONGOING_RESTORE,
    HTTP_HEADER_RESTORE_EXPIRE_DATE,
    HTTP_HEADER_CRC64ECMA,
];

fn is_kept_header(key_lower: &str) -> bool {
    PREDEFINED.contains(&key_lower) || key_lower.starts_with(XIAOMI_META_PREFIX)
}

/// A decimal `u64` made of ASCII digits only; anything else is `None`.
pub(crate) fn parse_decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// An HTTP date in the IMF-fixdate form (`Sun, 06 Nov 1994 08:49:37 GMT`);
/// the obsolete RFC 850 and asctime forms are `None`.
pub(crate) fn parse_http_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

/// User-defined object metadata: the `x-xiaomi-meta-*` entries, keyed
/// without that prefix.
///
/// Keys are stored ASCII-lowercase, so lookups and inserts ignore case; the
/// SDK adds the `x-xiaomi-meta-` prefix when it sends them. Iteration is in
/// key order.
///
/// Keys and values are checked when the request is sent, not on insert;
/// a failed check makes `send` (or `build`) fail with
/// [`ErrorKind::InvalidInput`](crate::ErrorKind::InvalidInput) before
/// anything is sent. Every request rejects an empty key and a key that
/// already starts with `x-xiaomi-meta-` (ignoring case). For requests that
/// carry the metadata as headers, `x-xiaomi-meta-{key}` must also be a valid
/// header name and the value a valid header value;
/// [`set_object_metadata`](crate::Client::set_object_metadata) sends a JSON
/// body and accepts any other key or value string.
///
/// A key is only the part after the prefix: `insert("owner", ..)` sends
/// `x-xiaomi-meta-owner`. A key named like a system header (for example
/// `content-type`) is still user metadata and travels as
/// `x-xiaomi-meta-content-type`; it never changes the system header. The
/// keys FDS reserves for system attributes (`content-length`,
/// `server-side-encryption`, `storage-class`, `ongoing-restore`,
/// `restore-expiry`, `hash-crc64ecma`) are sent as given, but a read reports
/// them in [`ObjectAttributes`], not in [`ObjectMetadata::user_metadata`].
///
/// ```
/// use fds::object::UserMetadata;
///
/// let mut metadata = UserMetadata::new();
/// metadata.insert("Owner", "upload-service");
/// assert_eq!(metadata.get("owner"), Some("upload-service"));
/// let metadata: UserMetadata = [("team", "storage")].into_iter().collect();
/// assert_eq!(metadata.len(), 1);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserMetadata {
    entries: BTreeMap<String, String>,
}

impl UserMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace an entry; returns the previous value for the key.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        let mut key = key.into();
        key.make_ascii_lowercase();
        self.entries.insert(key, value.into())
    }

    /// Case-insensitive lookup by key without the prefix. Missing and
    /// present-but-empty values differ.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .get(&key.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Remove an entry; returns its value.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.entries.remove(&key.to_ascii_lowercase())
    }

    /// The entries in key order, keys without the prefix.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Reject an empty key and a key that already carries the prefix (keys
    /// are stored lowercase).
    pub(crate) fn check_keys(&self) -> Result<()> {
        for key in self.entries.keys() {
            if key.is_empty() {
                return Err(Error::invalid_input("user metadata key must not be empty"));
            }
            if key.starts_with(XIAOMI_META_PREFIX) {
                return Err(Error::invalid_input(format!(
                    "user metadata key {key:?} must not include the {XIAOMI_META_PREFIX} prefix"
                )));
            }
        }
        Ok(())
    }

    /// Append every entry as an `x-xiaomi-meta-*` request header, checking
    /// the key, name and value.
    pub(crate) fn append_to_headers(&self, headers: &mut HeaderMap) -> Result<()> {
        self.check_keys()?;
        for (key, value) in self.iter() {
            let name = HeaderName::from_bytes(format!("{XIAOMI_META_PREFIX}{key}").as_bytes())
                .map_err(|e| Error::invalid_input(e.to_string()))?;
            headers.append(name, crate::client::header_value(value)?);
        }
        Ok(())
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for UserMetadata {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut metadata = Self::new();
        metadata.extend(iter);
        metadata
    }
}

impl<K: Into<String>, V: Into<String>> Extend<(K, V)> for UserMetadata {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}

/// The system attributes an object reports, typed. Read-only: they are
/// parsed from a response, never sent.
///
/// Every field is `None` when the response does not carry it. A numeric or
/// date value that does not parse also reads as `None` instead of failing
/// the request; [`ObjectMetadata::entries`] keeps the value as received.
/// String fields are verbatim.
///
/// | Field | Header or basic-info key |
/// | --- | --- |
/// | `content_type` | `content-type` |
/// | `content_length` | `content-length` |
/// | `content_encoding` | `content-encoding` |
/// | `cache_control` | `cache-control` |
/// | `content_md5` | `content-md5` |
/// | `last_modified` | `last-modified` |
/// | `etag` | `etag` |
/// | `storage_class` | `x-xiaomi-meta-storage-class` |
/// | `server_side_encryption` | `x-xiaomi-meta-server-side-encryption` |
/// | `crc64ecma` | `x-xiaomi-meta-hash-crc64ecma` |
/// | `meta_content_length` | `x-xiaomi-meta-content-length` |
/// | `ongoing_restore` | `x-xiaomi-meta-ongoing-restore` |
/// | `restore_expiry` | `x-xiaomi-meta-restore-expiry` |
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ObjectAttributes {
    pub content_type: Option<String>,
    /// The HTTP `Content-Length` of the response that carried the metadata:
    /// the object size for HEAD, the body length for GET (the range length
    /// of a partial response). Decimal digits only.
    pub content_length: Option<u64>,
    pub content_encoding: Option<String>,
    pub cache_control: Option<String>,
    pub content_md5: Option<String>,
    /// Parsed as an HTTP date in the IMF-fixdate form
    /// (`Sun, 06 Nov 1994 08:49:37 GMT`); other forms read as `None`.
    pub last_modified: Option<DateTime<Utc>>,
    /// Verbatim, including any quotes.
    pub etag: Option<String>,
    pub storage_class: Option<StorageClass>,
    pub server_side_encryption: Option<String>,
    /// The CRC-64/ECMA hash the service records, verbatim.
    pub crc64ecma: Option<String>,
    /// The length the service records in `x-xiaomi-meta-content-length`
    /// (what the Go SDK reports as the metadata content length). Unlike
    /// [`content_length`](Self::content_length) it does not describe the
    /// response that carried it. Decimal digits only.
    pub meta_content_length: Option<u64>,
    /// Whether an archive restoration is in progress, verbatim.
    pub ongoing_restore: Option<String>,
    /// When a restored archive copy expires, verbatim.
    pub restore_expiry: Option<String>,
}

impl ObjectAttributes {
    fn parse<'a>(value_of: impl Fn(&str) -> Option<&'a str>) -> Self {
        let text = |name: &str| value_of(name).map(str::to_owned);
        Self {
            content_type: text(HTTP_HEADER_CONTENT_TYPE),
            content_length: value_of(HTTP_HEADER_CONTENT_LENGTH).and_then(parse_decimal),
            content_encoding: text(HTTP_HEADER_CONTENT_ENCODING),
            cache_control: text(HTTP_HEADER_CACHE_CONTROL),
            content_md5: text(HTTP_HEADER_CONTENT_MD5),
            last_modified: value_of(HTTP_HEADER_LAST_MODIFIED).and_then(parse_http_date),
            etag: text(ETAG.as_str()),
            storage_class: value_of(HTTP_HEADER_STORAGE_CLASS).map(StorageClass::from),
            server_side_encryption: text(HTTP_HEADER_SERVER_SIDE_ENCRYPTION),
            crc64ecma: text(HTTP_HEADER_CRC64ECMA),
            meta_content_length: value_of(HTTP_HEADER_CONTENT_METADATA_LENGTH)
                .and_then(parse_decimal),
            ongoing_restore: text(HTTP_HEADER_ONGOING_RESTORE),
            restore_expiry: text(HTTP_HEADER_RESTORE_EXPIRE_DATE),
        }
    }
}

/// Object metadata as the service reports it: the typed system
/// [`attributes`](Self::attributes) and the
/// [`user_metadata`](Self::user_metadata).
///
/// Returned by [`head_object`](crate::Client::head_object) and
/// [`get_object_metadata`](crate::Client::get_object_metadata), by
/// [`GetObjectOutput::metadata`](crate::object::GetObjectOutput::metadata)
/// (all three read the response headers) and in
/// [`ObjectBasicInfo::metadata`](crate::object::ObjectBasicInfo::metadata).
/// To write metadata, use the typed builder methods and
/// [`UserMetadata`]; this type is never sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ObjectMetadata {
    pub attributes: ObjectAttributes,
    /// The `x-xiaomi-meta-*` entries without the prefix, except the keys
    /// listed in [`ObjectAttributes`].
    pub user_metadata: UserMetadata,
    entries: BTreeMap<String, String>,
}

impl ObjectMetadata {
    /// Every metadata entry as received, keys lowercase, in key order, for
    /// display and diagnostics; prefer [`attributes`](Self::attributes) and
    /// [`user_metadata`](Self::user_metadata).
    ///
    /// From response headers these are the protocol metadata headers
    /// (`content-type`, `content-length`, `cache-control`, `last-modified`,
    /// `date`, ...) and every `x-xiaomi-meta-*` header, a value that is not
    /// visible ASCII reading as empty; `etag` is not among them. From basic
    /// information they are every entry. A repeated key keeps its last value.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Parse the response headers of HEAD, GET or GET `?metadata`.
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        let mut entries = BTreeMap::new();
        for (name, value) in headers {
            // reqwest names are already lowercase.
            if is_kept_header(name.as_str()) {
                entries.insert(
                    name.as_str().to_owned(),
                    value.to_str().unwrap_or("").to_owned(),
                );
            }
        }
        let attributes = ObjectAttributes::parse(|name| {
            headers
                .get_all(name)
                .iter()
                .next_back()
                .and_then(|value| value.to_str().ok())
        });
        Self::assemble(attributes, entries)
    }

    fn from_entries(entries: BTreeMap<String, String>) -> Self {
        let attributes = ObjectAttributes::parse(|name| entries.get(name).map(String::as_str));
        Self::assemble(attributes, entries)
    }

    fn assemble(attributes: ObjectAttributes, entries: BTreeMap<String, String>) -> Self {
        let user_metadata = entries
            .iter()
            .filter(|(key, _)| !SYSTEM_META_KEYS.contains(&key.as_str()))
            .filter_map(|(key, value)| Some((key.strip_prefix(XIAOMI_META_PREFIX)?, value)))
            .map(|(key, value)| (key.to_owned(), value.clone()))
            .collect();
        Self {
            attributes,
            user_metadata,
            entries,
        }
    }
}

/// Deserialize the entry-list form of `ObjectBasicInfo.metadataBean`:
/// `{"rawMeta":{"entry":[{key,value}...]}}`. A `null` or missing bean or
/// `rawMeta` reads as empty metadata; keys are lowercased and a repeated key
/// keeps its last value.
pub(crate) fn entry_list<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ObjectMetadata, D::Error> {
    #[derive(Deserialize)]
    struct Entry {
        key: String,
        value: String,
    }
    #[derive(Deserialize, Default)]
    struct RawMeta {
        #[serde(default)]
        entry: Vec<Entry>,
    }
    #[derive(Deserialize, Default)]
    struct Aux {
        #[serde(
            rename = "rawMeta",
            default,
            deserialize_with = "crate::de::null_default"
        )]
        raw_meta: RawMeta,
    }

    let aux = Option::<Aux>::deserialize(deserializer)?.unwrap_or_default();
    let mut entries = BTreeMap::new();
    for kv in aux.raw_meta.entry {
        entries.insert(kv.key.to_ascii_lowercase(), kv.value);
    }
    Ok(ObjectMetadata::from_entries(entries))
}

/// The metadata a write request sends: the typed system headers and the
/// user metadata. Shared by the put, multipart, presign and
/// set-metadata builders.
#[derive(Debug, Clone, Default)]
pub(crate) struct WriteMetadata {
    pub(crate) cache_control: Option<String>,
    pub(crate) content_encoding: Option<String>,
    pub(crate) content_type: Option<String>,
    pub(crate) content_md5: Option<String>,
    pub(crate) user: UserMetadata,
}

impl WriteMetadata {
    /// The typed system headers in send order; empty values are not sent.
    pub(crate) fn system_headers(&self) -> [(&'static str, Option<&str>); 4] {
        [
            (HTTP_HEADER_CACHE_CONTROL, self.cache_control.as_deref()),
            (
                HTTP_HEADER_CONTENT_ENCODING,
                self.content_encoding.as_deref(),
            ),
            (HTTP_HEADER_CONTENT_TYPE, self.content_type.as_deref()),
            (HTTP_HEADER_CONTENT_MD5, self.content_md5.as_deref()),
        ]
    }

    /// Insert the non-empty typed system headers.
    pub(crate) fn insert_system_headers(&self, headers: &mut HeaderMap) -> Result<()> {
        for (name, value) in self.system_headers() {
            crate::object::insert_header_opt(headers, name, value)?;
        }
        Ok(())
    }

    /// The `set_object_metadata` body `{"rawMeta": {k:v,...}}`: every typed
    /// value that is set (an empty one included) and every user entry with
    /// its prefix, in key order.
    pub(crate) fn to_body(&self) -> Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Wrap {
            #[serde(rename = "rawMeta")]
            raw_meta: BTreeMap<String, String>,
        }
        let mut raw_meta = BTreeMap::new();
        for (name, value) in self.system_headers() {
            if let Some(value) = value {
                raw_meta.insert(name.to_owned(), value.to_owned());
            }
        }
        self.user.check_keys()?;
        for (key, value) in self.user.iter() {
            raw_meta.insert(format!("{XIAOMI_META_PREFIX}{key}"), value.to_owned());
        }
        serde_json::to_vec(&Wrap { raw_meta }).map_err(Error::json_encode)
    }
}

/// The typed system-header setters and `user_metadata` for a builder with a
/// `metadata: WriteMetadata` field. `$empty` documents what an empty value
/// does for that request.
macro_rules! write_metadata_methods {
    ($empty:literal) => {
        #[doc = concat!("The `Cache-Control` of the object. ", $empty)]
        pub fn cache_control(mut self, value: impl Into<String>) -> Self {
            self.metadata.cache_control = Some(value.into());
            self
        }

        #[doc = concat!("The `Content-Encoding` of the object. ", $empty)]
        pub fn content_encoding(mut self, value: impl Into<String>) -> Self {
            self.metadata.content_encoding = Some(value.into());
            self
        }

        #[doc = concat!(
                            "The `Content-Type` of the object; not inferred from the object name. ",
                            $empty
                        )]
        pub fn content_type(mut self, value: impl Into<String>) -> Self {
            self.metadata.content_type = Some(value.into());
            self
        }

        #[doc = concat!("The `Content-MD5` header. ", $empty)]
        pub fn content_md5(mut self, value: impl Into<String>) -> Self {
            self.metadata.content_md5 = Some(value.into());
            self
        }

        /// User metadata, sent as `x-xiaomi-meta-*` entries; replaces any
        /// earlier call. See [`UserMetadata`](crate::object::UserMetadata)
        /// for the key rules.
        pub fn user_metadata(mut self, metadata: crate::object::UserMetadata) -> Self {
            self.metadata.user = metadata;
            self
        }
    };
}
pub(crate) use write_metadata_methods;

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&str, &[u8])]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_bytes(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn user_metadata_normalizes_case() {
        let mut m = UserMetadata::new();
        assert_eq!(m.insert("Owner", "a"), None);
        assert_eq!(m.insert("OWNER", "b").as_deref(), Some("a"));
        assert_eq!(m.get("owner"), Some("b"));
        assert_eq!(m.iter().collect::<Vec<_>>(), [("owner", "b")]);
        assert_eq!(m.remove("Owner").as_deref(), Some("b"));
        assert!(m.is_empty());
    }

    #[test]
    fn user_metadata_header_names_are_checked_on_send() {
        let mut m = UserMetadata::new();
        m.insert("bad key", "v");
        let err = m.append_to_headers(&mut HeaderMap::new()).unwrap_err();
        assert_eq!(err.kind(), crate::ErrorKind::InvalidInput);
        let mut m = UserMetadata::new();
        m.insert("k", "bad\nvalue");
        let err = m.append_to_headers(&mut HeaderMap::new()).unwrap_err();
        assert_eq!(err.kind(), crate::ErrorKind::InvalidInput);
    }

    #[test]
    fn empty_and_prefixed_keys_are_rejected_on_every_transport() {
        for key in ["", "x-xiaomi-meta-a", "X-Xiaomi-Meta-a", "x-xiaomi-meta-"] {
            let metadata = WriteMetadata {
                user: [(key, "v")].into_iter().collect(),
                ..Default::default()
            };
            let err = metadata
                .user
                .append_to_headers(&mut HeaderMap::new())
                .unwrap_err();
            assert_eq!(err.kind(), crate::ErrorKind::InvalidInput, "{key:?}");
            let err = metadata.to_body().unwrap_err();
            assert_eq!(err.kind(), crate::ErrorKind::InvalidInput, "{key:?}");
        }
        // A key that merely contains the prefix later on is fine.
        let user: UserMetadata = [("a-x-xiaomi-meta-", "v")].into_iter().collect();
        user.append_to_headers(&mut HeaderMap::new()).unwrap();
    }

    #[test]
    fn body_is_the_sorted_map_form() {
        let metadata = WriteMetadata {
            content_type: Some("text/plain".into()),
            cache_control: Some(String::new()),
            user: [("b", "2"), ("A", "1"), ("content-type", "user")]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            String::from_utf8(metadata.to_body().unwrap()).unwrap(),
            r#"{"rawMeta":{"cache-control":"","content-type":"text/plain","x-xiaomi-meta-a":"1","x-xiaomi-meta-b":"2","x-xiaomi-meta-content-type":"user"}}"#
        );
        assert_eq!(
            WriteMetadata::default().to_body().unwrap(),
            br#"{"rawMeta":{}}"#
        );
    }

    #[test]
    fn system_headers_skip_empty_values_and_user_keys_stay_prefixed() {
        let metadata = WriteMetadata {
            content_type: Some("text/plain".into()),
            content_encoding: Some(String::new()),
            user: [("content-type", "user")].into_iter().collect(),
            ..Default::default()
        };
        let mut map = HeaderMap::new();
        metadata.insert_system_headers(&mut map).unwrap();
        metadata.user.append_to_headers(&mut map).unwrap();
        let sent: Vec<_> = map
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_str().unwrap()))
            .collect();
        assert_eq!(
            sent,
            [
                ("content-type", "text/plain"),
                ("x-xiaomi-meta-content-type", "user")
            ]
        );
    }

    #[test]
    fn headers_split_into_attributes_and_user_metadata() {
        let map = headers(&[
            ("content-type", b"image/png"),
            ("content-length", b"42"),
            ("content-encoding", b"gzip"),
            ("cache-control", b"no-cache"),
            ("content-md5", b"abc"),
            ("last-modified", b"Sun, 06 Nov 1994 08:49:37 GMT"),
            ("etag", b"\"tag\""),
            ("date", b"Mon, 13 Jul 2026 10:00:00 GMT"),
            ("x-xiaomi-meta-storage-class", b"STANDARD_IA"),
            ("x-xiaomi-meta-server-side-encryption", b"AES256"),
            ("x-xiaomi-meta-hash-crc64ecma", b"123"),
            ("x-xiaomi-meta-content-length", b"7"),
            ("x-xiaomi-meta-ongoing-restore", b"true"),
            ("x-xiaomi-meta-restore-expiry", b"1"),
            ("x-xiaomi-meta-owner", b"me"),
            ("x-xiaomi-meta-content-type", b"user"),
            ("server", b"ignored"),
        ]);
        let m = ObjectMetadata::from_headers(&map);
        let a = &m.attributes;
        assert_eq!(a.content_type.as_deref(), Some("image/png"));
        assert_eq!(a.content_length, Some(42));
        assert_eq!(a.content_encoding.as_deref(), Some("gzip"));
        assert_eq!(a.cache_control.as_deref(), Some("no-cache"));
        assert_eq!(a.content_md5.as_deref(), Some("abc"));
        assert_eq!(a.last_modified, DateTime::from_timestamp(784111777, 0));
        assert_eq!(a.etag.as_deref(), Some("\"tag\""));
        assert_eq!(a.storage_class.as_ref().unwrap().as_str(), "STANDARD_IA");
        assert_eq!(a.server_side_encryption.as_deref(), Some("AES256"));
        assert_eq!(a.crc64ecma.as_deref(), Some("123"));
        assert_eq!(a.meta_content_length, Some(7));
        assert_eq!(a.ongoing_restore.as_deref(), Some("true"));
        assert_eq!(a.restore_expiry.as_deref(), Some("1"));
        assert_eq!(
            m.user_metadata.iter().collect::<Vec<_>>(),
            [("content-type", "user"), ("owner", "me")]
        );
        // Entries keep the whitelisted headers verbatim; etag and server are not among them.
        let entries: BTreeMap<_, _> = m.entries().collect();
        assert_eq!(entries.len(), 15);
        assert_eq!(entries["content-length"], "42");
        assert_eq!(entries["date"], "Mon, 13 Jul 2026 10:00:00 GMT");
        assert!(!entries.contains_key("etag"));
        assert!(!entries.contains_key("server"));
    }

    #[test]
    fn missing_and_invalid_values_read_as_none() {
        let m = ObjectMetadata::from_headers(&HeaderMap::new());
        assert_eq!(m, ObjectMetadata::default());

        let map = headers(&[
            ("content-length", b"-1"),
            ("last-modified", b"yesterday"),
            ("x-xiaomi-meta-content-length", b"1e3"),
            ("content-type", b"caf\xc3\xa9"),
            ("x-xiaomi-meta-name", b"\xff"),
        ]);
        let m = ObjectMetadata::from_headers(&map);
        assert_eq!(m.attributes.content_length, None);
        assert_eq!(m.attributes.last_modified, None);
        assert_eq!(m.attributes.meta_content_length, None);
        // Not visible ASCII: no typed value; the entry reads as empty.
        assert_eq!(m.attributes.content_type, None);
        assert_eq!(m.user_metadata.get("name"), Some(""));
        let entries: BTreeMap<_, _> = m.entries().collect();
        assert_eq!(entries["content-length"], "-1");
        assert_eq!(entries["last-modified"], "yesterday");
        assert_eq!(entries["content-type"], "");
    }

    #[test]
    fn repeated_headers_keep_the_last_value() {
        let map = headers(&[("x-xiaomi-meta-k", b"1"), ("x-xiaomi-meta-k", b"2")]);
        let m = ObjectMetadata::from_headers(&map);
        assert_eq!(m.user_metadata.get("k"), Some("2"));
        assert_eq!(m.entries().collect::<Vec<_>>(), [("x-xiaomi-meta-k", "2")]);
    }

    #[derive(Deserialize)]
    struct Bean {
        #[serde(deserialize_with = "entry_list", default)]
        metadata: ObjectMetadata,
    }

    #[test]
    fn entry_list_form() {
        let json = r#"{"metadata":{"rawMeta":{"entry":[
            {"key":"Content-Type","value":"image/png"},
            {"key":"x-xiaomi-meta-uid","value":"1000"},
            {"key":"x-xiaomi-meta-UID","value":"1001"},
            {"key":"x-xiaomi-meta-storage-class","value":"Archive"},
            {"key":"etag","value":"e"},
            {"key":"vendor-key","value":"kept"}]}}}"#;
        let m = serde_json::from_str::<Bean>(json).unwrap().metadata;
        assert_eq!(m.attributes.content_type.as_deref(), Some("image/png"));
        assert_eq!(m.attributes.etag.as_deref(), Some("e"));
        assert!(m.attributes.storage_class.as_ref().unwrap().is_archive());
        assert_eq!(
            m.user_metadata.iter().collect::<Vec<_>>(),
            [("uid", "1001")]
        );
        // Every entry is kept, keys lowercased, last value wins.
        assert_eq!(
            m.entries().collect::<Vec<_>>(),
            [
                ("content-type", "image/png"),
                ("etag", "e"),
                ("vendor-key", "kept"),
                ("x-xiaomi-meta-storage-class", "Archive"),
                ("x-xiaomi-meta-uid", "1001"),
            ]
        );
    }

    #[test]
    fn entry_list_null_and_missing_read_as_empty() {
        for json in [
            r#"{}"#,
            r#"{"metadata":null}"#,
            r#"{"metadata":{}}"#,
            r#"{"metadata":{"rawMeta":null}}"#,
            r#"{"metadata":{"rawMeta":{}}}"#,
        ] {
            let m = serde_json::from_str::<Bean>(json).unwrap().metadata;
            assert_eq!(m, ObjectMetadata::default(), "{json}");
        }
        for json in [
            r#"{"metadata":{"rawMeta":{"entry":null}}}"#,
            r#"{"metadata":{"rawMeta":{"entry":[{"key":"k"}]}}}"#,
            r#"{"metadata":{"rawMeta":{"entry":[{"key":"k","value":null}]}}}"#,
        ] {
            assert!(serde_json::from_str::<Bean>(json).is_err(), "{json}");
        }
    }
}
