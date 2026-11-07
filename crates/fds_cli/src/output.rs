//! CLI-owned JSON views. SDK response types need not expose serialization contracts.
use chrono::{DateTime, Utc};
use fds::{bucket::*, oauth::*, object::*, *};
use serde_json::{json, Value};

/// The CLI's time format: Unix epoch milliseconds, `0` when absent.
fn millis(time: Option<DateTime<Utc>>) -> i64 {
    time.map_or(0, |time| time.timestamp_millis())
}
pub fn bucket(v: &BucketInfo) -> Value {
    json!({"name":v.bucket_name, "allowOutsideAccess":v.allow_outside_access,
        "enableSSE":v.enable_sse, "creationTime":millis(v.creation_time), "numObjects":v.object_count,
        "usedSpace":v.used_space, "storageClass":v.storage_class})
}
pub fn buckets(v: &BucketList) -> Value {
    json!({"owner":v.owner,"buckets":v.buckets.iter().map(bucket).collect::<Vec<_>>()})
}
pub fn app_buckets(v: &AppBucketList) -> Value {
    json!({"owner":v.owner, "buckets":v.buckets.iter().map(|b| json!({
        "name":b.bucket_name,"orgId":b.org_id,"creationTime":b.creation_time.timestamp_millis(),
        "usedSpace":b.used_space,"numObjects":b.object_count
    })).collect::<Vec<_>>()})
}
pub fn put(v: &PutObjectOutput) -> Value {
    // Keep the full verification response. Like presigned URLs, it may contain a signature.
    json!({"bucketName":v.bucket_name,"objectName":v.object_name,"accessKeyId":v.access_key_id,
        "signature":v.signature,"expires":millis(v.expires),
        "previousVersionId":v.previous_version_id.as_deref().unwrap_or_default(),"outsideAccess":v.outside_access})
}
pub fn basic(v: &ObjectBasicInfo) -> Value {
    json!({"allowOutsideAccess":v.allow_outside_access,"size":v.size,"uploadTime":millis(v.upload_time),
        "storageClass":v.storage_class,
        "metadata":v.metadata.entries().collect::<std::collections::BTreeMap<_, _>>()})
}
pub fn listing(v: &ObjectListing) -> Value {
    json!({"name":v.bucket_name,"prefix":v.prefix,"maxKeys":v.max_keys,"marker":v.marker,
        "truncated":v.truncated,"nextMarker":v.next_marker,"delimiter":v.delimiter,
        "commonPrefixes":v.common_prefixes,"objects":v.objects.iter().map(|o| json!({
            "name":o.object_name,"etag":o.etag,"owner":o.owner,"size":o.size,
            "lastModified":o.last_modified,"uploadTime":millis(o.upload_time)
        })).collect::<Vec<_>>()})
}
pub fn deleted(v: &DeleteObjectsOutput) -> Value {
    json!({"deleted":v.deleted,"failures":v.failures.iter().map(|f| json!({
        "object_name":f.object_name,"error_code":f.error_code,"error_description":f.error_description
    })).collect::<Vec<_>>()})
}
/// `batch` is the explicit name list of a single-batch command; it is reported
/// as `not_submitted` when the SDK did not send that batch.
pub fn delete_result(
    result: fds::Result<DeleteObjectsOutput>,
    batch: &[String],
) -> anyhow::Result<()> {
    match result {
        Ok(output) => delete_outcome(output, None, &[]),
        Err(error) => {
            let output = error.partial_delete_output().cloned().unwrap_or_default();
            let not_submitted = if error.dispatch() == Dispatch::NotSent {
                batch
            } else {
                &[]
            };
            delete_outcome(output, Some(error), not_submitted)
        }
    }
}
/// Print a bulk-deletion outcome. Per-object failures are reported as an error
/// even when the SDK returned them in `Ok`, so the JSON and the exit code do
/// not depend on how the SDK classifies them.
pub fn delete_outcome(
    output: DeleteObjectsOutput,
    error: Option<fds::Error>,
    not_submitted: &[String],
) -> anyhow::Result<()> {
    let error = match error {
        Some(error) => error,
        None if output.failures.is_empty() => return super::util::print_json(&deleted(&output)),
        None => partial_failure(&output),
    };
    super::util::print_json(
        &json!({"output":deleted(&output),"unconfirmed":output.unconfirmed,"not_submitted":not_submitted}),
    )?;
    Err(error.into())
}
/// The error reported for one batch with per-object failures.
pub fn partial_failure(batch: &DeleteObjectsOutput) -> fds::Error {
    fds::Error::new(
        ErrorKind::PartialFailure,
        "bulk deletion failed for some objects",
    )
    .with_context("deleted", batch.deleted.len())
    .with_context("failed", batch.failures.len())
}
pub fn ok() -> anyhow::Result<()> {
    super::util::print_json(&json!({"ok":true}))
}
