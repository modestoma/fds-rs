//! Protocol constants. **Values are byte-exact** (they match the reference
//! Go SDK). Crate-internal: the spelling and
//! casing are protocol-sensitive implementation details, not public API.

// ---- Header names (lowercase, for case-insensitive matching) --------------
pub(crate) const XIAOMI_PREFIX: &str = "x-xiaomi-";
pub(crate) const XIAOMI_META_PREFIX: &str = "x-xiaomi-meta-";

// Presign query parameter names — CASE-SENSITIVE (do not lowercase).
pub(crate) const HTTP_HEADER_GALAXY_ACCESS_KEY_ID: &str = "GalaxyAccessKeyId";
pub(crate) const HTTP_HEADER_SIGNATURE: &str = "Signature";
pub(crate) const HTTP_HEADER_EXPIRES: &str = "Expires";

pub(crate) const HTTP_HEADER_CACHE_CONTROL: &str = "cache-control";
pub(crate) const HTTP_HEADER_CONTENT_LENGTH: &str = "content-length";
pub(crate) const HTTP_HEADER_CONTENT_ENCODING: &str = "content-encoding";
pub(crate) const HTTP_HEADER_LAST_MODIFIED: &str = "last-modified";
pub(crate) const HTTP_HEADER_CONTENT_MD5: &str = "content-md5";
pub(crate) const HTTP_HEADER_CONTENT_TYPE: &str = "content-type";
pub(crate) const HTTP_HEADER_LAST_CHECKED: &str = "last-checked";
pub(crate) const HTTP_HEADER_UPLOAD_TIME: &str = "upload-time";
pub(crate) const HTTP_HEADER_DATE: &str = "date";
pub(crate) const HTTP_HEADER_AUTHORIZATION: &str = "authorization";
pub(crate) const HTTP_HEADER_RANGE: &str = "range";
pub(crate) const HTTP_HEADER_CONTENT_RANGE: &str = "content-range";

// Composite header names (prefix + name). Note the subtle values:
pub(crate) const HTTP_HEADER_CONTENT_METADATA_LENGTH: &str = "x-xiaomi-meta-content-length";
pub(crate) const HTTP_HEADER_SERVER_SIDE_ENCRYPTION: &str = "x-xiaomi-meta-server-side-encryption";
pub(crate) const HTTP_HEADER_STORAGE_CLASS: &str = "x-xiaomi-meta-storage-class";
pub(crate) const HTTP_HEADER_ONGOING_RESTORE: &str = "x-xiaomi-meta-ongoing-restore";
/// Constant name says ExpireDate but the value is `restore-expiry`.
pub(crate) const HTTP_HEADER_RESTORE_EXPIRE_DATE: &str = "x-xiaomi-meta-restore-expiry";
pub(crate) const HTTP_HEADER_CRC64ECMA: &str = "x-xiaomi-meta-hash-crc64ecma";
/// `x-xiaomi-` prefix (NOT `x-xiaomi-meta-`): enters the signature's canonical
/// xiaomi headers but is not user metadata.
pub(crate) const HTTP_HEADER_MULTIPART_UPLOAD_MODE: &str = "x-xiaomi-multipart-upload-mode";

// ---- Miscellaneous constants ----------------------------------------------
pub(crate) const DEFAULT_LIST_OBJECTS_MAX_KEYS: std::num::NonZeroU32 =
    std::num::NonZeroU32::new(1000).unwrap();
