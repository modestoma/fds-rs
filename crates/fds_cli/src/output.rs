//! CLI-owned JSON views. SDK response types need not expose serialization contracts.
use fds::*;
use serde_json::{json, Value};

pub fn bucket(v: &GetBucketInfoResponse) -> Value {
    json!({"name":v.bucket_name, "allowOutsideAccess":v.allow_outside_access,
        "enableSSE":v.enable_sse, "creationTime":v.creation_time, "numObjects":v.object_num,
        "usedSpace":v.used_space, "storageClass":v.storage_class_type})
}
pub fn buckets(v: &ListBucketsResponse) -> Value {
    json!({"owner":v.owner,"buckets":v.buckets.iter().map(bucket).collect::<Vec<_>>()})
}
pub fn app_buckets(v: &AppBuckets) -> Value {
    json!({"owner":v.owner, "buckets":v.buckets.iter().map(|b| json!({
        "name":b.name,"orgId":b.org_id,"creationTime":b.creation_time,
        "usedSpace":b.used_space,"numObjects":b.num_objects
    })).collect::<Vec<_>>()})
}
pub fn put(v: &PutObjectResponse) -> Value {
    // Keep the full verification response. Like presigned URLs, it may contain a signature.
    json!({"bucketName":v.bucket_name,"objectName":v.object_name,"accessKeyId":v.access_key_id,
        "signature":v.signature,"expires":v.expires,"previousVersionId":v.previous_version_id,"outsideAccess":v.outside_access})
}
pub fn basic(v: &ObjectBasicInfo) -> Value {
    json!({"allowOutsideAccess":v.allow_outside_access,"size":v.size,"uploadTime":v.upload_time,
        "storageClass":v.storage_class_type,"metadata":v.metadata.as_map()})
}
pub fn listing(v: &ObjectListing) -> Value {
    json!({"name":v.bucket_name,"prefix":v.prefix,"maxKeys":v.max_keys,"marker":v.marker,
        "truncated":v.truncated,"nextMarker":v.next_marker,"delimiter":v.delimiter,
        "commonPrefixes":v.common_prefixes,"objects":v.object_summaries.iter().map(|o| json!({
            "name":o.object_name,"etag":o.etag,"owner":o.owner,"size":o.size,
            "lastModified":o.last_modified,"uploadTime":o.upload_time
        })).collect::<Vec<_>>()})
}
pub fn deleted(v: &DeleteObjectsOutput) -> Value {
    json!({"deleted":v.deleted,"failures":v.failures.iter().map(|f| json!({
        "object_name":f.object_name,"error_code":f.error_code,"error_description":f.error_description
    })).collect::<Vec<_>>()})
}
pub fn delete_result(
    result: std::result::Result<DeleteObjectsOutput, DeleteObjectsError>,
) -> anyhow::Result<()> {
    match result {
        Ok(output) => super::util::print_json(&deleted(&output)),
        Err(error) => {
            super::util::print_json(
                &json!({"output":deleted(&error.output),"unconfirmed":error.unconfirmed,"not_submitted":error.not_submitted}),
            )?;
            Err(error.into())
        }
    }
}
pub fn ok() -> anyhow::Result<()> {
    super::util::print_json(&json!({"ok":true}))
}
