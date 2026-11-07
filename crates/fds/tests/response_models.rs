//! Wire compatibility of every response model: missing keys, `null` values and
//! unknown keys. Lenient models read `null` exactly like a missing key (the
//! field default); strict models reject both for required fields. Unknown keys
//! are always ignored.

use fds::acl::{AccessControlList, Grant, GrantKey, Owner};
use fds::bucket::{
    AccessLog, GetBucketInfoResponse, LifecycleBaseItem, LifecycleConfig, LifecycleRule,
    ListBucketsResponse,
};
use fds::multipart::UploadPartResponse;
use fds::oauth::{AppBucket, AppBuckets};
use fds::object::{
    CopyObjectResponse, DeleteObjectFailure, ObjectBasicInfo, ObjectListing, ObjectMetadata,
    ObjectSummary, PutObjectResponse,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use std::fmt::Debug;

fn parse<T: DeserializeOwned>(value: &Value) -> serde_json::Result<T> {
    // Through bytes, as responses are decoded.
    serde_json::from_slice(value.to_string().as_bytes())
}

fn debug<T: DeserializeOwned + Debug>(value: &Value) -> String {
    format!("{:?}", parse::<T>(value).unwrap())
}

fn object(map: &Map<String, Value>) -> Value {
    Value::Object(map.clone())
}

fn snake_case(key: &str) -> String {
    let mut out = String::new();
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn with_unknown_keys(map: &Map<String, Value>) -> Value {
    let mut map = map.clone();
    map.insert("unknownField".into(), json!({"nested": [1, null, "x"]}));
    map.insert("UNKNOWN".into(), Value::Null);
    Value::Object(map)
}

/// `full` sets every wire key to a non-default value.
fn lenient<T: DeserializeOwned + Default + Debug>(full: Value) {
    let name = std::any::type_name::<T>();
    let default = format!("{:?}", T::default());
    let map = full.as_object().unwrap();
    // Every key missing.
    assert_eq!(debug::<T>(&json!({})), default, "{name}: missing keys");
    // Every key null.
    let nulls = map.keys().map(|k| (k.clone(), Value::Null)).collect();
    assert_eq!(
        debug::<T>(&Value::Object(nulls)),
        default,
        "{name}: null keys"
    );
    // Unknown keys are ignored.
    let base = debug::<T>(&full);
    assert_eq!(
        debug::<T>(&with_unknown_keys(map)),
        base,
        "{name}: unknown keys"
    );
    for key in map.keys() {
        let mut missing = map.clone();
        missing.remove(key);
        let mut null = map.clone();
        null.insert(key.clone(), Value::Null);
        let missing = debug::<T>(&object(&missing));
        assert_eq!(
            missing,
            debug::<T>(&object(&null)),
            "{name}.{key}: null != missing"
        );
        // Every wire key populates a field: the wire names are exact.
        assert_ne!(missing, base, "{name}.{key}: wire key ignored");
        // The snake_case spelling is just another unknown key.
        let snake = snake_case(key);
        if snake != *key {
            let only = json!({ snake: map[key].clone() });
            assert_eq!(
                debug::<T>(&only),
                default,
                "{name}.{key}: snake_case accepted"
            );
        }
    }
}

/// Required keys reject both absence and `null`; unknown keys are ignored.
fn strict<T: DeserializeOwned + Debug>(full: Value, required: &[&str]) {
    let name = std::any::type_name::<T>();
    let map = full.as_object().unwrap();
    assert_eq!(
        debug::<T>(&with_unknown_keys(map)),
        debug::<T>(&full),
        "{name}: unknown keys"
    );
    for key in required {
        assert!(map.contains_key(*key), "{name}.{key}: not in sample");
        let mut missing = map.clone();
        missing.remove(*key);
        assert!(
            parse::<T>(&object(&missing)).is_err(),
            "{name}.{key}: missing accepted"
        );
        let mut null = map.clone();
        null.insert(key.to_string(), Value::Null);
        assert!(
            parse::<T>(&object(&null)).is_err(),
            "{name}.{key}: null accepted"
        );
    }
}

#[test]
fn object_models() {
    lenient::<PutObjectResponse>(json!({
        "bucketName": "b", "objectName": "o", "accessKeyId": "ak", "signature": "sig",
        "expires": 1, "previousVersionId": "v1", "outsideAccess": true,
    }));
    lenient::<ObjectBasicInfo>(json!({
        "allowOutsideAccess": false, "size": 10, "uploadTime": 2, "storageClass": "Standard",
        "metadataBean": {"rawMeta": {"entry": [{"key": "content-type", "value": "text/plain"}]}},
    }));
    lenient::<ObjectSummary>(json!({
        "etag": "e", "name": "p/a", "owner": {"id": "i", "displayName": "d"}, "size": 3,
        "lastModified": "2024-01-02T03:04:05Z", "uploadTime": 4,
    }));
    lenient::<ObjectListing>(json!({
        "name": "b", "prefix": "p/", "maxKeys": 10, "marker": "m", "truncated": true,
        "nextMarker": "n", "delimiter": "/", "objects": [{"name": "p/a"}],
        "commonPrefixes": ["p/d/"],
    }));
    strict::<CopyObjectResponse>(
        json!({"bucketName": "b", "objectName": "o"}),
        &["bucketName", "objectName"],
    );
}

#[test]
fn delete_failure_is_strict_snake_case() {
    // The bulk-delete failure array uses snake_case keys; Debug is redacted,
    // so compare values.
    let full = json!({"object_name": "o", "error_code": "c", "error_description": "d"});
    let parsed: DeleteObjectFailure = parse(&full).unwrap();
    assert_eq!(
        (parsed.object_name.as_str(), parsed.error_code.as_str()),
        ("o", "c")
    );
    assert_eq!(parsed.error_description, "d");
    let unknown: DeleteObjectFailure =
        parse(&with_unknown_keys(full.as_object().unwrap())).unwrap();
    assert!(unknown == parsed);
    strict::<DeleteObjectFailure>(full, &["object_name", "error_code", "error_description"]);
    assert!(parse::<DeleteObjectFailure>(&json!({
        "objectName": "o", "errorCode": "c", "errorDescription": "d"
    }))
    .is_err());
}

#[test]
fn object_metadata_model() {
    let empty = format!("{:?}", ObjectMetadata::new());
    assert_eq!(debug::<ObjectMetadata>(&json!({})), empty);
    assert_eq!(debug::<ObjectMetadata>(&json!({"rawMeta": null})), empty);
    assert_eq!(debug::<ObjectMetadata>(&json!({"rawMeta": {}})), empty);
    let full = json!({"rawMeta": {"entry": [{"key": "Content-Type", "value": "a/b"}]}});
    let parsed: ObjectMetadata = parse(&full).unwrap();
    assert_eq!(parsed.get("content-type"), Some("a/b"));
    let unknown = json!({"rawMeta": {"entry": [{"key": "content-type", "value": "a/b", "x": 1}], "y": 2}, "z": 3});
    assert_eq!(parse::<ObjectMetadata>(&unknown).unwrap(), parsed);
    // Only `rawMeta` itself is null-tolerant.
    assert!(parse::<ObjectMetadata>(&json!({"rawMeta": {"entry": null}})).is_err());
    assert!(parse::<ObjectMetadata>(&json!({"rawMeta": {"entry": [{"key": "k"}]}})).is_err());
}

#[test]
fn multipart_models() {
    strict::<UploadPartResponse>(
        json!({"partNumber": 1, "etag": "e", "partSize": 5}),
        &["partNumber", "etag", "partSize"],
    );
}

#[test]
fn bucket_models() {
    lenient::<GetBucketInfoResponse>(json!({
        "allowOutsideAccess": true, "enableSSE": true, "creationTime": 1, "name": "b",
        "numObjects": 2, "usedSpace": 3, "storageClass": "STANDARD",
    }));
    lenient::<ListBucketsResponse>(json!({
        "owner": {"id": "i"}, "buckets": [{"name": "b"}],
    }));
    lenient::<LifecycleRule>(json!({
        "id": "r", "prefix": "p", "enabled": true, "actions": {"expiration": {"days": 1.5}},
    }));
    lenient::<LifecycleConfig>(json!({"rules": [{"id": "r"}]}));
    lenient::<AccessLog>(json!({
        "bucketName": "b", "enabled": true, "logBucketName": "l", "logPrefix": "p",
    }));
    strict::<LifecycleBaseItem>(json!({"days": 1.5}), &["days"]);
}

#[test]
fn acl_models() {
    lenient::<Owner>(json!({"id": "i", "displayName": "d"}));
    lenient::<GrantKey>(json!({"id": "i", "displayName": "d"}));
    lenient::<AccessControlList>(json!({
        "accessControlList": [{"grantee": {"id": "i"}, "permission": "READ", "type": "USER"}],
        "owner": {"id": "o"},
    }));
    // Grant: the grantee is lenient, permission and type are required.
    let full =
        json!({"grantee": {"id": "i", "displayName": "d"}, "permission": "WRITE", "type": "GROUP"});
    strict::<Grant>(full.clone(), &["permission", "type"]);
    let mut grant = full.as_object().unwrap().clone();
    grant.remove("grantee");
    let missing: Grant = parse(&object(&grant)).unwrap();
    grant.insert("grantee".into(), Value::Null);
    let null: Grant = parse(&object(&grant)).unwrap();
    assert_eq!(missing, null);
    assert_eq!(missing.grantee, GrantKey::default());
    assert_ne!(parse::<Grant>(&full).unwrap(), missing);
}

#[test]
fn oauth_models_are_strict() {
    let bucket = json!({
        "name": "b", "orgId": "o", "creationTime": 1, "usedSpace": 2, "numObjects": 3,
    });
    strict::<AppBucket>(
        bucket.clone(),
        &["name", "orgId", "creationTime", "usedSpace", "numObjects"],
    );
    strict::<AppBuckets>(
        json!({"owner": {"id": "i"}, "buckets": [bucket]}),
        &["owner", "buckets"],
    );
}
