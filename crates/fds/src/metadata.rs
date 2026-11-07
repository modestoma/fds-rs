//! Object metadata with its three (de)serialization forms. Ports the
//! `ObjectMetadata` section of `fds/object.go`.
//!
//! The three forms are asymmetric — mixing them fails parsing:
//! 1. **send** (`serialize`, SetObjectMetadata body) = `{"rawMeta": {k:v,...}}` (map);
//! 2. **receive** (`Deserialize`, ObjectBasicInfo.metadataBean)
//!    = `{"rawMeta":{"entry":[{key,value},...]}}` (entry-list);
//! 3. **GetObjectMetadata** (`from_headers`) reads the response headers.

use std::collections::HashMap;

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
use crate::error::{FdsError, Result};

/// The allowed "predefined" metadata keys (object.go:302-322, all lowercase).
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

fn is_allowed_key(key_lower: &str) -> bool {
    PREDEFINED.contains(&key_lower) || key_lower.starts_with(XIAOMI_META_PREFIX)
}

/// Object metadata (Go `ObjectMetadata`). Keys are stored lowercase.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectMetadata {
    metadata: HashMap<String, String>,
}

impl ObjectMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    /// Case-insensitive lookup. Missing and present-but-empty values differ.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.metadata
            .get(&key.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Borrow metadata without cloning its keys or values.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.metadata.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Set a value. Only predefined keys or `x-xiaomi-meta-*` are accepted;
    /// otherwise an `Invalid metadata: {k}` error (Go `Set`).
    pub fn set(&mut self, k: &str, v: impl Into<String>) -> Result<()> {
        let key = k.to_ascii_lowercase();
        if is_allowed_key(&key) {
            self.metadata.insert(key, v.into());
            Ok(())
        } else {
            Err(FdsError::invalid(format!("Invalid metadata: {k}")))
        }
    }

    /// Borrow the complete raw map without allocation. Clone explicitly if needed.
    pub fn as_map(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    pub fn is_empty(&self) -> bool {
        self.metadata.is_empty()
    }

    pub fn set_content_length(&mut self, length: i64) {
        let _ = self.set(HTTP_HEADER_CONTENT_METADATA_LENGTH, length.to_string());
    }

    pub fn get_content_length(&self) -> Result<i64> {
        self.get(HTTP_HEADER_CONTENT_METADATA_LENGTH)
            .ok_or_else(|| FdsError::invalid("missing content metadata length"))?
            .parse::<i64>()
            .map_err(|e| FdsError::invalid(e.to_string()))
    }

    pub fn set_content_type(&mut self, content_type: impl Into<String>) {
        let _ = self.set(HTTP_HEADER_CONTENT_TYPE, content_type);
    }

    pub fn get_content_type(&self) -> Option<&str> {
        self.get(HTTP_HEADER_CONTENT_TYPE)
    }

    /// Serialize as the SetObjectMetadata request body: `{"rawMeta": {k:v,...}}`
    /// (Go `serialize`).
    pub fn serialize(&self) -> Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Wrap<'a> {
            #[serde(rename = "rawMeta")]
            raw_meta: &'a HashMap<String, String>,
        }
        Ok(serde_json::to_vec(&Wrap {
            raw_meta: &self.metadata,
        })?)
    }

    /// Build from HTTP response headers (Go `parseObjectMetadataFromHeader`),
    /// used by GetObjectMetadata.
    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        let mut m = Self::new();
        for (name, value) in headers.iter() {
            let key = name.as_str().to_ascii_lowercase(); // reqwest is already lowercase
            if is_allowed_key(&key) {
                let _ = m.set(&key, value.to_str().unwrap_or(""));
            }
        }
        m
    }
}

/// Deserialize the entry-list form used by `ObjectBasicInfo.metadataBean`
/// (Go `ObjectMetadata.UnmarshalJSON`): `{"rawMeta":{"entry":[{key,value}...]}}`.
impl<'de> Deserialize<'de> for ObjectMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
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

        let aux = Aux::deserialize(deserializer)?;
        let mut metadata = HashMap::with_capacity(aux.raw_meta.entry.len());
        for kv in aux.raw_meta.entry {
            metadata.insert(kv.key.to_ascii_lowercase(), kv.value);
        }
        Ok(ObjectMetadata { metadata })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_accepts_predefined_and_xiaomi_meta() {
        let mut m = ObjectMetadata::new();
        assert!(m.set("x-xiaomi-meta-uid", "1").is_ok());
        assert!(m.set("Content-Type", "image/png").is_ok()); // case-insensitive
        assert_eq!(m.get("content-type"), Some("image/png"));
    }

    #[test]
    fn set_rejects_unknown_key() {
        let mut m = ObjectMetadata::new();
        let err = m.set("foo", "1").unwrap_err();
        assert_eq!(err.to_string(), "Invalid metadata: foo");
    }

    #[test]
    fn serialize_map_form() {
        let mut m = ObjectMetadata::new();
        m.set("content-type", "text/plain").unwrap();
        let bytes = m.serialize().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["rawMeta"]["content-type"], "text/plain");
    }

    #[test]
    fn deserialize_entry_list_form() {
        let json = r#"{"rawMeta":{"entry":[{"key":"content-type","value":"image/png"},{"key":"x-xiaomi-meta-uid","value":"1000"}]}}"#;
        let m: ObjectMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(m.get("content-type"), Some("image/png"));
        assert_eq!(m.get("x-xiaomi-meta-uid"), Some("1000"));
    }

    #[test]
    fn content_length_helpers() {
        let mut m = ObjectMetadata::new();
        m.set_content_length(4096);
        assert_eq!(m.get_content_length().unwrap(), 4096);
    }
}
