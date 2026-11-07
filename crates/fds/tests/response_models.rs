//! Wire compatibility of every response model: missing keys, `null` values and
//! unknown keys. Lenient models read `null` exactly like a missing key (the
//! field default); strict models reject both for required fields. Unknown keys
//! are always ignored.
//!
//! Typed fields: millisecond timestamps become `DateTime<Utc>`, sizes and
//! counts unsigned integers; a negative size, a negative or unrepresentable
//! timestamp or a value of the wrong JSON type is rejected, and a response
//! decoder reports that as `ErrorKind::ProtocolViolation`.

use chrono::{DateTime, Utc};
use fds::acl::{AccessControlList, Grant, GrantKey, Owner};
use fds::bucket::{
    AccessLog, BucketInfo, BucketList, LifecycleBaseItem, LifecycleConfig, LifecycleRule,
};
use fds::multipart::UploadPartOutput;
use fds::oauth::{AppBucket, AppBucketList};
use fds::object::StorageClass;
use fds::object::{
    CopyObjectOutput, DeleteObjectFailure, ObjectBasicInfo, ObjectListing, ObjectMetadata,
    ObjectSummary, PutObjectOutput, UserMetadata,
};
use fds::{Client, ErrorKind};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use std::fmt::Debug;
use std::num::NonZeroU16;

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
    lenient::<PutObjectOutput>(json!({
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
    strict::<CopyObjectOutput>(
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

/// The entry-list metadata form, read through `ObjectBasicInfo.metadataBean`.
fn bean(metadata: Value) -> serde_json::Result<ObjectMetadata> {
    parse::<ObjectBasicInfo>(&json!({ "metadataBean": metadata })).map(|info| info.metadata)
}

#[test]
fn object_metadata_model() {
    let empty = ObjectMetadata::default();
    assert_eq!(bean(json!({})).unwrap(), empty);
    assert_eq!(bean(json!(null)).unwrap(), empty);
    assert_eq!(bean(json!({"rawMeta": null})).unwrap(), empty);
    assert_eq!(bean(json!({"rawMeta": {}})).unwrap(), empty);
    let full = json!({"rawMeta": {"entry": [{"key": "Content-Type", "value": "a/b"}]}});
    let parsed = bean(full).unwrap();
    assert_eq!(parsed.attributes.content_type.as_deref(), Some("a/b"));
    assert!(parsed.user_metadata.is_empty());
    let unknown = json!({"rawMeta": {"entry": [{"key": "content-type", "value": "a/b", "x": 1}], "y": 2}, "z": 3});
    assert_eq!(bean(unknown).unwrap(), parsed);
    // Only `rawMeta` itself is null-tolerant.
    assert!(bean(json!({"rawMeta": {"entry": null}})).is_err());
    assert!(bean(json!({"rawMeta": {"entry": [{"key": "k"}]}})).is_err());
}

#[test]
fn object_metadata_entry_list_splits_attributes_and_user_metadata() {
    let parsed = bean(json!({"rawMeta": {"entry": [
        {"key": "content-type", "value": "text/plain"},
        {"key": "content-length", "value": "12"},
        {"key": "last-modified", "value": "Sun, 06 Nov 1994 08:49:37 GMT"},
        {"key": "x-xiaomi-meta-content-length", "value": "not a number"},
        {"key": "x-xiaomi-meta-storage-class", "value": "Standard"},
        {"key": "x-xiaomi-meta-hash-crc64ecma", "value": "99"},
        {"key": "X-Xiaomi-Meta-Owner", "value": "me"},
        {"key": "x-xiaomi-meta-content-type", "value": "user"},
    ]}}))
    .unwrap();
    let a = &parsed.attributes;
    assert_eq!(a.content_type.as_deref(), Some("text/plain"));
    assert_eq!(a.content_length, Some(12));
    assert_eq!(a.last_modified, DateTime::from_timestamp(784_111_777, 0));
    // An invalid number reads as `None`; the entry keeps it.
    assert_eq!(a.meta_content_length, None);
    assert!(a.storage_class.as_ref().unwrap().is_standard());
    assert_eq!(a.crc64ecma.as_deref(), Some("99"));
    let user: UserMetadata = [("owner", "me"), ("content-type", "user")]
        .into_iter()
        .collect();
    assert_eq!(parsed.user_metadata, user);
    assert_eq!(parsed.entries().count(), 8);
    assert!(parsed
        .entries()
        .any(|entry| entry == ("x-xiaomi-meta-content-length", "not a number")));
}

#[test]
fn multipart_models() {
    strict::<UploadPartOutput>(
        json!({"partNumber": 1, "etag": "e", "partSize": 5}),
        &["partNumber", "etag", "partSize"],
    );
}

#[test]
fn bucket_models() {
    lenient::<BucketInfo>(json!({
        "allowOutsideAccess": true, "enableSSE": true, "creationTime": 1, "name": "b",
        "numObjects": 2, "usedSpace": 3, "storageClass": "STANDARD",
    }));
    lenient::<BucketList>(json!({
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
    strict::<AppBucketList>(
        json!({"owner": {"id": "i"}, "buckets": [bucket]}),
        &["owner", "buckets"],
    );
}

fn at(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap()
}

/// 9999-12-31T23:59:59.999Z.
const LATE: i64 = 253_402_300_799_999;

#[test]
fn typed_fields_convert() {
    // Millisecond timestamps, including 0 and a far-future instant.
    for millis in [0, 1, 1_700_000_000_123, LATE] {
        let put: PutObjectOutput = parse(&json!({"expires": millis})).unwrap();
        assert_eq!(put.expires, Some(at(millis)), "{millis}");
        let info: ObjectBasicInfo = parse(&json!({"uploadTime": millis})).unwrap();
        assert_eq!(info.upload_time, Some(at(millis)), "{millis}");
        let summary: ObjectSummary = parse(&json!({"uploadTime": millis})).unwrap();
        assert_eq!(summary.upload_time, Some(at(millis)), "{millis}");
        let bucket: BucketInfo = parse(&json!({"creationTime": millis})).unwrap();
        assert_eq!(bucket.creation_time, Some(at(millis)), "{millis}");
        let app: AppBucket = parse(&json!({
            "name": "b", "orgId": "o", "creationTime": millis, "usedSpace": 0, "numObjects": 0,
        }))
        .unwrap();
        assert_eq!(app.creation_time, at(millis), "{millis}");
    }
    assert_eq!(
        at(1_700_000_000_123).to_rfc3339(),
        "2023-11-14T22:13:20.123+00:00"
    );
    // Sizes and counts: the full unsigned range, beyond i64::MAX.
    for size in [0, 1, i64::MAX as u64 + 1, u64::MAX] {
        let info: ObjectBasicInfo = parse(&json!({"size": size})).unwrap();
        assert_eq!(info.size, size);
        let summary: ObjectSummary = parse(&json!({"size": size})).unwrap();
        assert_eq!(summary.size, size);
        let bucket: BucketInfo = parse(&json!({"numObjects": size, "usedSpace": size})).unwrap();
        assert_eq!((bucket.object_count, bucket.used_space), (size, size));
        let part: UploadPartOutput =
            parse(&json!({"partNumber": 1, "etag": "e", "partSize": size})).unwrap();
        assert_eq!(part.part_size, size);
    }
    let listing: ObjectListing = parse(&json!({"maxKeys": u32::MAX})).unwrap();
    assert_eq!(listing.max_keys, u32::MAX);
    for number in [1, 10_000, u16::MAX] {
        let part: UploadPartOutput =
            parse(&json!({"partNumber": number, "etag": "e", "partSize": 1})).unwrap();
        assert_eq!(part.part_number, NonZeroU16::new(number).unwrap());
    }
    // Renamed fields keep their wire names.
    let bucket: BucketInfo = parse(&json!({
        "name": "b", "numObjects": 2, "storageClass": "Standard", "enableSSE": true,
    }))
    .unwrap();
    assert_eq!(bucket.bucket_name, "b");
    assert_eq!(bucket.object_count, 2);
    assert!(bucket
        .storage_class
        .as_ref()
        .is_some_and(StorageClass::is_standard));
    assert!(bucket.enable_sse);
    let listing: ObjectListing = parse(&json!({"objects": [{"name": "a", "size": 1}]})).unwrap();
    assert_eq!(listing.objects[0].object_name, "a");
    let info: ObjectBasicInfo = parse(&json!({"storageClass": "ARCHIVE"})).unwrap();
    assert!(info.storage_class.unwrap().is_archive());
    let app: AppBucket = parse(&json!({
        "name": "b", "orgId": "o", "creationTime": 1, "usedSpace": 2, "numObjects": 3,
    }))
    .unwrap();
    assert_eq!((app.bucket_name.as_str(), app.object_count), ("b", 3));
}

#[test]
fn optional_fields_distinguish_missing_from_empty() {
    // Missing and null are None; a present value, even an empty one, is kept.
    for body in [json!({}), json!({"previousVersionId": null})] {
        let put: PutObjectOutput = parse(&body).unwrap();
        assert_eq!(put.previous_version_id, None, "{body}");
    }
    let put: PutObjectOutput = parse(&json!({"previousVersionId": ""})).unwrap();
    assert_eq!(put.previous_version_id.as_deref(), Some(""));
    let put: PutObjectOutput = parse(&json!({"previousVersionId": "v1"})).unwrap();
    assert_eq!(put.previous_version_id.as_deref(), Some("v1"));
    // Time fields: missing and null are None.
    for body in [
        json!({}),
        json!({"uploadTime": null, "creationTime": null, "expires": null}),
    ] {
        assert_eq!(parse::<ObjectSummary>(&body).unwrap().upload_time, None);
        assert_eq!(parse::<ObjectBasicInfo>(&body).unwrap().upload_time, None);
        assert_eq!(parse::<BucketInfo>(&body).unwrap().creation_time, None);
        assert_eq!(parse::<PutObjectOutput>(&body).unwrap().expires, None);
    }
}

/// Values every time field rejects.
fn bad_times() -> Vec<Value> {
    vec![
        json!(-1),
        json!(i64::MIN),
        // Beyond the representable range of DateTime<Utc>.
        json!(9_000_000_000_000_000_i64),
        json!(i64::MAX),
        json!(u64::MAX),
        json!(1.5),
        json!("1700000000000"),
        json!(true),
    ]
}

/// Values every size or count field rejects.
fn bad_sizes() -> Vec<Value> {
    vec![
        json!(-1),
        json!(i64::MIN),
        json!(1.5),
        json!("10"),
        json!(false),
    ]
}

type Parser = fn(&Value) -> bool;

fn accepts<T: DeserializeOwned>(value: &Value) -> bool {
    parse::<T>(value).is_ok()
}

#[test]
fn invalid_times_sizes_and_part_numbers_are_rejected() {
    let app =
        json!({"name": "b", "orgId": "o", "creationTime": 1, "usedSpace": 2, "numObjects": 3});
    let part = json!({"partNumber": 1, "etag": "e", "partSize": 5});
    // (model, valid sample, key, rejected values)
    let cases: Vec<(&str, Parser, Value, &str, Vec<Value>)> = vec![
        (
            "PutObjectOutput",
            accepts::<PutObjectOutput>,
            json!({}),
            "expires",
            bad_times(),
        ),
        (
            "ObjectBasicInfo",
            accepts::<ObjectBasicInfo>,
            json!({}),
            "uploadTime",
            bad_times(),
        ),
        (
            "ObjectBasicInfo",
            accepts::<ObjectBasicInfo>,
            json!({}),
            "size",
            bad_sizes(),
        ),
        (
            "ObjectSummary",
            accepts::<ObjectSummary>,
            json!({}),
            "uploadTime",
            bad_times(),
        ),
        (
            "ObjectSummary",
            accepts::<ObjectSummary>,
            json!({}),
            "size",
            bad_sizes(),
        ),
        (
            "ObjectListing",
            accepts::<ObjectListing>,
            json!({}),
            "maxKeys",
            {
                let mut values = bad_sizes();
                values.push(json!(u64::from(u32::MAX) + 1));
                values
            },
        ),
        (
            "BucketInfo",
            accepts::<BucketInfo>,
            json!({}),
            "creationTime",
            bad_times(),
        ),
        (
            "BucketInfo",
            accepts::<BucketInfo>,
            json!({}),
            "numObjects",
            bad_sizes(),
        ),
        (
            "BucketInfo",
            accepts::<BucketInfo>,
            json!({}),
            "usedSpace",
            bad_sizes(),
        ),
        (
            "AppBucket",
            accepts::<AppBucket>,
            app.clone(),
            "creationTime",
            bad_times(),
        ),
        (
            "AppBucket",
            accepts::<AppBucket>,
            app.clone(),
            "usedSpace",
            bad_sizes(),
        ),
        (
            "AppBucket",
            accepts::<AppBucket>,
            app,
            "numObjects",
            bad_sizes(),
        ),
        (
            "UploadPartOutput",
            accepts::<UploadPartOutput>,
            part.clone(),
            "partSize",
            bad_sizes(),
        ),
        (
            "UploadPartOutput",
            accepts::<UploadPartOutput>,
            part,
            "partNumber",
            {
                let mut values = bad_sizes();
                values.extend([json!(0), json!(65_536)]);
                values
            },
        ),
    ];
    for (model, accepts, sample, key, values) in cases {
        assert!(accepts(&sample), "{model}: sample rejected");
        for value in values {
            let mut map = sample.as_object().unwrap().clone();
            map.insert(key.into(), value.clone());
            assert!(!accepts(&object(&map)), "{model}.{key}: {value} accepted");
        }
    }
    // Nested values fail the enclosing response.
    assert!(parse::<ObjectListing>(&json!({"objects": [{"size": -1}]})).is_err());
    assert!(parse::<BucketList>(&json!({"buckets": [{"creationTime": -1}]})).is_err());
}

/// Responses with invalid typed values are protocol violations, not
/// transport or decode errors of another kind.
#[tokio::test]
async fn invalid_typed_values_are_protocol_violations() {
    let mut server = mockito::Server::new_async().await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials("AKID", "sk")
        .build()
        .unwrap();
    // (method, path, body); the index selects the operation below.
    let cases = [
        (
            "GET",
            "/b",
            r#"{"name":"b","objects":[{"name":"a","size":-1}]}"#,
        ),
        (
            "GET",
            "/b",
            r#"{"name":"b","objects":[{"name":"a","uploadTime":-5}]}"#,
        ),
        (
            "GET",
            "/b/o",
            r#"{"size":1,"uploadTime":9000000000000000000}"#,
        ),
        ("GET", "/b", r#"{"name":"b","numObjects":-2}"#),
        (
            "PUT",
            "/b/o",
            r#"{"bucketName":"b","objectName":"o","expires":-1}"#,
        ),
        ("PUT", "/b/o", r#"{"partNumber":0,"etag":"e","partSize":1}"#),
    ];
    for (index, (method, path, body)) in cases.into_iter().enumerate() {
        let mock = server
            .mock(method, path)
            .match_query(mockito::Matcher::Any)
            .with_body(body)
            .create_async()
            .await;
        let error = match index {
            0 | 1 => client.list_objects("b").send().await.map(drop),
            2 => client
                .get_object_basic_info("b", "o")
                .send()
                .await
                .map(drop),
            3 => client.get_bucket_info("b").send().await.map(drop),
            4 => client
                .put_object("b", "o", b"x".to_vec())
                .send()
                .await
                .map(drop),
            _ => client
                .upload_part("b", "o", "u", NonZeroU16::MIN, b"x".to_vec())
                .send()
                .await
                .map(drop),
        }
        .unwrap_err();
        assert_eq!(
            error.kind(),
            ErrorKind::ProtocolViolation,
            "{body}: {error:?}"
        );
        mock.assert_async().await;
        mock.remove_async().await;
    }
}
