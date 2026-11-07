use super::util::parse_meta;
use anyhow::{anyhow, bail, Result};
use fds::*;
use std::time::{SystemTime, UNIX_EPOCH};
struct Report {
    pass: usize,
    fail: usize,
}

impl Report {
    fn new() -> Self {
        Self { pass: 0, fail: 0 }
    }
    fn ok(&mut self, label: &str) {
        println!("  \x1b[32m✓\x1b[0m {label}");
        self.pass += 1;
    }
    fn fail(&mut self, label: &str, detail: impl std::fmt::Display) {
        println!("  \x1b[31m✗\x1b[0m {label}: {detail}");
        self.fail += 1;
    }
    /// Report a `Result`, returning the value on success.
    fn check<T, E: std::fmt::Display>(&mut self, label: &str, r: Result<T, E>) -> Option<T> {
        match r {
            Ok(v) => {
                self.ok(label);
                Some(v)
            }
            Err(e) => {
                self.fail(label, e);
                None
            }
        }
    }
    /// Report a boolean assertion.
    fn assert(&mut self, label: &str, cond: bool) {
        if cond {
            self.ok(label);
        } else {
            self.fail(label, "assertion failed");
        }
    }
}

pub async fn smoke(
    client: &Client,
    bucket: &str,
    keep: bool,
    skip_multipart: bool,
    skip_download: bool,
) -> Result<()> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = format!("fds-smoke-{}-{ts}/", std::process::id());
    let hello = format!("{prefix}hello.txt");
    let copy = format!("{prefix}copy.txt");
    let renamed = format!("{prefix}renamed.txt");
    let multipart = format!("{prefix}multipart.bin");
    let content = b"hello fds cli smoke \xF0\x9F\x9A\x80"; // includes a rocket emoji
    let mut rep = Report::new();

    println!("smoke test against bucket '{bucket}', prefix '{prefix}'");

    // bucket reachability
    rep.check("does_bucket_exist", client.does_bucket_exist(bucket).await);

    // put + get + verify
    let put = client
        .put_object(
            PutObjectRequest::new(bucket, hello.clone(), content.to_vec())
                .content_type("text/plain")
                .metadata(parse_meta(&["x-xiaomi-meta-smoke=1".into()])?),
        )
        .await;
    rep.check("put_object", put);

    match async {
        client
            .get_object(&GetObjectRequest {
                bucket_name: bucket.into(),
                object_name: hello.clone(),
                range: None,
                timeout: None,
            })
            .await?
            .bytes()
            .await
    }
    .await
    {
        Ok(b) => rep.assert("get_object matches", b.as_ref() == content),
        Err(e) => rep.fail("get_object", e),
    }

    // range get
    match async {
        client
            .get_object(&GetObjectRequest {
                bucket_name: bucket.into(),
                object_name: hello.clone(),
                range: Some("bytes=0-4".parse().unwrap()),
                timeout: None,
            })
            .await?
            .bytes()
            .await
    }
    .await
    {
        Ok(b) => rep.assert("get_object range", b.as_ref() == &content[0..5]),
        Err(e) => rep.fail("get_object range", e),
    }

    rep.assert(
        "does_object_exist",
        client
            .does_object_exist(bucket, &hello)
            .await
            .unwrap_or(false),
    );

    // metadata read
    match client.get_object_metadata(bucket, &hello).await {
        Ok(md) => rep.assert(
            "get_object_metadata (smoke=1)",
            md.get("x-xiaomi-meta-smoke") == Some("1"),
        ),
        Err(e) => rep.fail("get_object_metadata", e),
    }

    // metadata write + read back
    let set_md = client
        .set_object_metadata(&SetObjectMetadataRequest {
            bucket_name: bucket.into(),
            object_name: hello.clone(),
            metadata: parse_meta(&[
                "x-xiaomi-meta-smoke=2".into(),
                "content-type=text/plain".into(),
            ])?,
        })
        .await;
    rep.check("set_object_metadata", set_md);
    match client.get_object_metadata(bucket, &hello).await {
        Ok(md) => rep.assert(
            "metadata updated (smoke=2)",
            md.get("x-xiaomi-meta-smoke") == Some("2"),
        ),
        Err(e) => rep.fail("metadata updated", e),
    }

    // copy
    let cp = client
        .copy_object(&CopyObjectRequest {
            source_bucket_name: bucket.into(),
            source_object_name: hello.clone(),
            target_bucket_name: bucket.into(),
            target_object_name: copy.clone(),
        })
        .await;
    rep.check("copy_object", cp);
    rep.assert(
        "copy exists",
        client
            .does_object_exist(bucket, &copy)
            .await
            .unwrap_or(false),
    );

    // rename
    let rn = client.rename_object(bucket, &copy, &renamed).await;
    rep.check("rename_object", rn);
    rep.assert(
        "renamed exists & old gone",
        client
            .does_object_exist(bucket, &renamed)
            .await
            .unwrap_or(false)
            && !client
                .does_object_exist(bucket, &copy)
                .await
                .unwrap_or(true),
    );

    // list
    match client
        .list_objects(&ListObjectsRequest {
            bucket_name: bucket.into(),
            prefix: prefix.clone(),
            ..Default::default()
        })
        .await
    {
        Ok(l) => rep.assert("list_objects sees >=2", l.object_summaries.len() >= 2),
        Err(e) => rep.fail("list_objects", e),
    }

    // basic info
    match client.get_object_basic_info(bucket, &hello).await {
        Ok(info) => rep.assert(
            "basic_info size matches",
            info.size as usize == content.len(),
        ),
        Err(e) => rep.fail("basic_info", e),
    }

    // ACL: make public, then check a grant exists
    let pub_res = client.set_object_public(bucket, &hello).await;
    rep.check("set_object_public", pub_res);
    match client
        .get_object_acl(&GetObjectACLRequest {
            bucket_name: bucket.into(),
            object_name: hello.clone(),
            version_id: None,
        })
        .await
    {
        Ok(acl) => rep.assert("object acl has grants", !acl.grants.is_empty()),
        Err(e) => rep.fail("get_object_acl", e),
    }

    // presigned + absolute URL (local)
    match client.generate_presigned_url(&GeneratePresignedURLRequest {
        cdn: false,
        bucket_name: bucket.into(),
        object_name: hello.clone(),
        method: HttpMethod::Get,
        expiration: chrono::Utc::now() + chrono::Duration::hours(1),
        metadata: None,
    }) {
        Ok(u) => rep.assert(
            "presigned url",
            u.starts_with("http") && u.contains("&Signature="),
        ),
        Err(e) => rep.fail("presigned url", e),
    }
    rep.assert(
        "absolute url",
        client
            .generate_absolute_object_url(bucket, &hello)
            .map(|u| u.contains(&hello.replace('/', "%2F")))
            .unwrap_or(false),
    );

    // download manager
    if !skip_download {
        let out = std::env::temp_dir().join(format!("fds-smoke-{ts}.bin"));
        let r = async {
            let d = Downloader::new(
                client.clone(),
                fds::DownloadOptions::builder()
                    .part_size(1024 * 1024)
                    .concurrency(4)
                    .resume(false)
                    .build()?,
            );
            d.download(&DownloadRequest::new(bucket, &hello, &out))
                .await
                .map_err(|e| anyhow!(e))?;
            let got = std::fs::read(&out)?;
            let _ = std::fs::remove_file(&out);
            if got == content {
                Ok(())
            } else {
                Err(anyhow!("downloaded content mismatch"))
            }
        }
        .await;
        rep.check("download (manager)", r);
    }

    // multipart (uploads ~6 MiB)
    if !skip_multipart {
        let data = vec![7u8; 6 * 1024 * 1024];
        let mp = multipart_put(client, bucket, &multipart, &data, 5 * 1024 * 1024).await;
        rep.check("multipart_upload", mp);
        match client.get_object_basic_info(bucket, &multipart).await {
            Ok(info) => rep.assert("multipart size == 6 MiB", info.size as usize == data.len()),
            Err(e) => rep.fail("multipart basic_info", e),
        }
    }

    // cleanup
    if !keep {
        let mut names = vec![hello.clone(), copy.clone(), renamed.clone()];
        if !skip_multipart {
            names.push(multipart.clone());
        }
        let del = client.delete_objects(bucket, &names, false).await;
        match del {
            Ok(_) => rep.assert("cleanup delete_objects", true),
            Err(e) => rep.fail("cleanup delete_objects", e),
        }
        rep.assert(
            "hello gone after delete",
            !client
                .does_object_exist(bucket, &hello)
                .await
                .unwrap_or(true),
        );
    } else {
        println!("  (kept objects under {prefix})");
    }

    println!("\n{} passed, {} failed", rep.pass, rep.fail);
    if rep.fail > 0 {
        bail!("{} smoke checks failed", rep.fail);
    }
    Ok(())
}

/// Small fixed test payload; real files use the managed multipart command.
async fn multipart_put(
    client: &Client,
    bucket: &str,
    object: &str,
    data: &[u8],
    part_size: usize,
) -> Result<PutObjectResponse> {
    let mut init = InitMultipartUploadRequest::new(bucket, object)
        .send(client)
        .await?;
    init.bucket_name = bucket.to_owned();
    init.object_name = object.to_owned();
    eprintln!(
        "smoke multipart session: {}",
        serde_json::json!({"bucket":bucket,"object":object,"upload_id":init.upload_id})
    );
    let mut parts = Vec::new();
    for (number, chunk) in (1i32..).zip(data.chunks(part_size)) {
        match init.part(number, chunk.to_vec()).send(client).await {
            Ok(part) => parts.push(part),
            Err(error) => {
                if let Err(cleanup) = init.abort(client).await {
                    eprintln!("smoke multipart abort failed: {cleanup}");
                }
                return Err(error.into());
            }
        }
    }
    match init.complete(parts.clone()).send(client).await {
        Ok(value) => Ok(value),
        Err(error) => {
            eprintln!(
                "smoke completion unknown; confirmed parts: {}",
                serde_json::to_string(&parts)?
            );
            Err(error.into())
        }
    }
}
