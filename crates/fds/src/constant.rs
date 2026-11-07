//! Constants ported verbatim from `fds/constant.go`. **Values are byte-exact**;
//! only names are Rust-ified. The spelling and casing are protocol-sensitive.

use serde::{Deserialize, Serialize};

// ---- Header names (lowercase, for case-insensitive matching) --------------
pub const XIAOMI_PREFIX: &str = "x-xiaomi-";
pub const XIAOMI_META_PREFIX: &str = "x-xiaomi-meta-";

// Presign query parameter names — CASE-SENSITIVE (do not lowercase).
pub const HTTP_HEADER_GALAXY_ACCESS_KEY_ID: &str = "GalaxyAccessKeyId";
pub const HTTP_HEADER_SIGNATURE: &str = "Signature";
pub const HTTP_HEADER_EXPIRES: &str = "Expires";

pub const HTTP_HEADER_CACHE_CONTROL: &str = "cache-control";
pub const HTTP_HEADER_CONTENT_LENGTH: &str = "content-length";
pub const HTTP_HEADER_CONTENT_ENCODING: &str = "content-encoding";
pub const HTTP_HEADER_LAST_MODIFIED: &str = "last-modified";
pub const HTTP_HEADER_CONTENT_MD5: &str = "content-md5";
pub const HTTP_HEADER_CONTENT_TYPE: &str = "content-type";
pub const HTTP_HEADER_LAST_CHECKED: &str = "last-checked";
pub const HTTP_HEADER_UPLOAD_TIME: &str = "upload-time";
pub const HTTP_HEADER_DATE: &str = "date";
pub const HTTP_HEADER_AUTHORIZATION: &str = "authorization";
pub const HTTP_HEADER_RANGE: &str = "range";
pub const HTTP_HEADER_CONTENT_RANGE: &str = "content-range";

// Composite header names (prefix + name). Note the subtle values:
pub const HTTP_HEADER_CONTENT_METADATA_LENGTH: &str = "x-xiaomi-meta-content-length";
pub const HTTP_HEADER_SERVER_SIDE_ENCRYPTION: &str = "x-xiaomi-meta-server-side-encryption";
pub const HTTP_HEADER_STORAGE_CLASS: &str = "x-xiaomi-meta-storage-class";
pub const HTTP_HEADER_ONGOING_RESTORE: &str = "x-xiaomi-meta-ongoing-restore";
/// Constant name says ExpireDate but the value is `restore-expiry`.
pub const HTTP_HEADER_RESTORE_EXPIRE_DATE: &str = "x-xiaomi-meta-restore-expiry";
pub const HTTP_HEADER_CRC64ECMA: &str = "x-xiaomi-meta-hash-crc64ecma";
/// `x-xiaomi-` prefix (NOT `x-xiaomi-meta-`): enters the signature's canonical
/// xiaomi headers but is not user metadata.
pub const HTTP_HEADER_MULTIPART_UPLOAD_MODE: &str = "x-xiaomi-multipart-upload-mode";

// ---- Miscellaneous constants ----------------------------------------------
pub const MAX_PART_SIZE: i64 = 5 * 1024 * 1024 * 1024; // 5 GiB
pub const MIN_PART_SIZE: i64 = 5 * 1024 * 1024; // 5 MiB
pub const FILE_PERM_MODE: u32 = 0o664;
/// Cosmetic only (does not affect protocol); Go used `fds-go-temp-`.
pub const TEMP_FILE_PREFIX: &str = "fds-rs-temp-";
pub const DEFAULT_LIST_OBJECTS_MAX_KEYS: i32 = 1000;

pub const URL_COM_SUFFIX: &str = ".fds.api.xiaomi.com";
/// NOTE: leading hyphen, not a dot.
pub const URL_NET_SUFFIX: &str = "-fds.api.xiaomi.net";
pub const URL_CDN_SUFFIX: &str = ".fds.api.mi-img.com";

/// Go SDK version (informational only; Rust does not set a User-Agent).
pub const VERSION: &str = "0.9.0";

// Multipart upload modes (constant.go:81-84). BUG-3: Go's `ModeDierect` is a
// typo; we fix the name to `MODE_DIRECT` but KEEP the value `"DIRECT"`.
pub const MODE_MULTI_BLOB: &str = "MULTI_BLOB";
pub const MODE_DIRECT: &str = "DIRECT";

/// HTTP request method (Go `HTTPMethod`). String values are byte-exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Put,
    Head,
    Post,
    Delete,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Put => "PUT",
            HttpMethod::Head => "HEAD",
            HttpMethod::Post => "POST",
            HttpMethod::Delete => "DELETE",
        }
    }
}

impl From<HttpMethod> for reqwest::Method {
    fn from(m: HttpMethod) -> Self {
        match m {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Head => reqwest::Method::HEAD,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Delete => reqwest::Method::DELETE,
        }
    }
}

/// Storage class. FDS models this as `type StorageClass string` (an opaque
/// string), and the value's casing is NOT stable: e.g. `create_bucket` sends
/// `STANDARD`, but `get_bucket_info` may return `Standard`. A strict enum would
/// reject those, so we keep a transparent string wrapper (any server value
/// deserializes and round-trips losslessly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StorageClass(pub String);

impl StorageClass {
    /// Canonical value used by `create_bucket` (matches Go's `Standard`).
    pub const STANDARD: &'static str = "STANDARD";
    pub const STANDARD_IA: &'static str = "STANDARD_IA";
    pub const ARCHIVE: &'static str = "Archive";

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for StorageClass {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for StorageClass {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_strings() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    }

    #[test]
    fn storage_class_transparent_string() {
        // Serializes/deserializes as a bare string, and accepts ANY casing the
        // server returns (Go treats it as an opaque string).
        assert_eq!(
            serde_json::to_string(&StorageClass::new("Archive")).unwrap(),
            "\"Archive\""
        );
        let sc: StorageClass = serde_json::from_str("\"Standard\"").unwrap(); // title-case from server
        assert_eq!(sc.as_str(), "Standard");
        let sc2: StorageClass = serde_json::from_str("\"STANDARD_IA\"").unwrap();
        assert_eq!(sc2.as_str(), "STANDARD_IA");
    }
}
