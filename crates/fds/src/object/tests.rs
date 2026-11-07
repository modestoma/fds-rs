use crate::Client;
use http::Method;
use std::time::Duration;

use super::*;

fn client() -> Client {
    Client::builder()
        .endpoint("https://cnbj1.fds.api.xiaomi.com")
        .cdn_endpoint("https://cdn.cnbj1.fds.api.mi-img.com")
        .credentials("AKID", "sk")
        .build()
        .unwrap()
}

#[test]
fn basic_info_entry_list_metadata() {
    // Server may return title-case (e.g. "Standard"); the string wrapper accepts it.
    let json = r#"{"size":10,"uploadTime":1,"storageClass":"Standard","metadataBean":{"rawMeta":{"entry":[{"key":"content-type","value":"image/png"}]}}}"#;
    let info: ObjectBasicInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.size, 10);
    assert_eq!(
        info.storage_class_type.as_ref().map(|s| s.as_str()),
        Some("Standard")
    );
    assert_eq!(info.metadata.get("content-type"), Some("image/png"));
}

#[test]
fn object_listing_tolerates_null_fields() {
    // FDS returns `null` (not absent) for marker/nextMarker/delimiter and for
    // empty slices; it reads as the default.
    let json = r#"{"name":"b","prefix":"manifest/","maxKeys":100,"marker":null,"truncated":false,"nextMarker":null,"delimiter":null,"objects":[{"name":"manifest/a","size":3,"owner":null,"lastModified":null}],"commonPrefixes":null}"#;
    let l: ObjectListing = serde_json::from_str(json).unwrap();
    assert_eq!(l.bucket_name, "b");
    assert_eq!(l.marker, "");
    assert_eq!(l.next_marker, "");
    assert_eq!(l.delimiter, "");
    assert!(l.common_prefixes.is_empty());
    assert_eq!(l.object_summaries.len(), 1);
    assert_eq!(l.object_summaries[0].object_name, "manifest/a");
    assert!(l.object_summaries[0].last_modified.is_none());
}

#[test]
fn presigned_get_url_shape() {
    let ts = chrono::DateTime::from_timestamp_millis(1_752_400_000_000).unwrap();
    let url = client()
        .presign_object("b", "o")
        .expires_at(ts)
        .build()
        .unwrap();
    // Query is sorted: Expires < GalaxyAccessKeyId; Signature appended raw.
    assert!(url.starts_with(
            "https://cnbj1.fds.api.xiaomi.com/b/o?Expires=1752400000000&GalaxyAccessKeyId=AKID&Signature="
        ));
    // No "metadata" subresource for GET.
    assert!(!url.contains("metadata"));
}

#[test]
fn presigned_head_url_has_metadata_subresource() {
    let ts = chrono::DateTime::from_timestamp_millis(1_752_400_000_000).unwrap();
    let url = client()
        .presign_object("b", "o")
        .method(Method::HEAD)
        .expires_at(ts)
        .build()
        .unwrap();
    assert!(url.contains("metadata="));
    assert!(url.contains("&Signature="));
}

#[test]
fn presign_requires_a_valid_expiration() {
    let missing = client().presign_object("b", "o").build().unwrap_err();
    assert_eq!(missing.kind(), crate::ErrorKind::InvalidInput);
    for lifetime in [Duration::ZERO, Duration::from_micros(999), Duration::MAX] {
        let error = client()
            .presign_object("b", "o")
            .expires_in(lifetime)
            .build()
            .unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::InvalidInput, "{lifetime:?}");
    }
    let url = client()
        .presign_object("b", "o")
        .expires_in(Duration::from_secs(60))
        .cdn()
        .build()
        .unwrap();
    assert!(url.starts_with("https://cdn.cnbj1.fds.api.mi-img.com/b/o?Expires="));
}

#[test]
fn absolute_url() {
    let u = client().absolute_object_url("b", "dir/o.txt").unwrap();
    assert_eq!(u, "https://cnbj1.fds.api.xiaomi.com/b/dir%2Fo.txt");
}

#[test]
fn storage_class_transparent_string() {
    // Serializes/deserializes as a bare string, and accepts ANY casing the
    // server returns (an opaque string).
    assert_eq!(
        serde_json::to_string(&StorageClass::new("Archive")).unwrap(),
        "\"Archive\""
    );
    let sc: StorageClass = serde_json::from_str("\"Standard\"").unwrap(); // title-case from server
    assert_eq!(sc.as_str(), "Standard");
    let sc2: StorageClass = serde_json::from_str("\"STANDARD_IA\"").unwrap();
    assert_eq!(sc2.as_str(), "STANDARD_IA");
}

#[test]
fn marker_guard_bounded_and_exact_modes() {
    use super::listing::MarkerGuard;
    for start in [None, Some("start")] {
        for mut guard in [MarkerGuard::new(start), MarkerGuard::exact(start)] {
            assert!(!guard.advance(""));
            assert!(guard.advance("a"));
            assert!(!guard.advance("a"), "adjacent repetition");
            assert!(guard.advance("b"));
            assert!(!guard.advance("a"), "repetition of the checkpoint");
        }
    }
    // A cycle back to the caller's marker after three pages: the bounded mode
    // only detects it later, the exact mode at once.
    let pages = ["a", "b", "start"];
    let mut bounded = MarkerGuard::new(Some("start"));
    assert!(pages.iter().all(|next| bounded.advance(next)));
    let mut exact = MarkerGuard::exact(Some("start"));
    assert!(exact.advance("a") && exact.advance("b"));
    assert!(!exact.advance("start"));
    // Bounded mode still terminates on that cycle.
    let mut cycle = ["a", "b", "start"].iter().cycle();
    assert!((0..16).any(|_| !bounded.advance(cycle.next().unwrap())));
}
