use super::output;
use super::util::{parse_method, print_json, read_json, AclMode, BodyArgs, MetadataArgs};
use anyhow::Result;
use clap::Subcommand;
use fds::{object::*, *};
#[derive(Subcommand)]
pub enum ObjectCmd {
    Put {
        bucket: String,
        object: String,
        #[command(flatten)]
        body: BodyArgs,
        #[command(flatten)]
        metadata: MetadataArgs,
        /// Response-signature expiration (RFC3339), not object TTL
        #[arg(long)]
        expires: Option<chrono::DateTime<chrono::Utc>>,
        #[arg(long)]
        expect: Option<String>,
        #[arg(long)]
        content_disposition: Option<String>,
    },
    Get {
        bucket: String,
        object: String,
        #[arg(long)]
        range: Option<String>,
        /// Print status and response headers as JSON to stderr
        #[arg(long)]
        show_headers: bool,
        /// Write body to this file (else raw bytes to stdout)
        #[arg(long)]
        out: Option<String>,
    },
    Head {
        bucket: String,
        object: String,
    },
    Prefetch {
        bucket: String,
        object: String,
    },
    Refresh {
        bucket: String,
        object: String,
    },
    Url {
        bucket: String,
        object: String,
    },
    Presign {
        bucket: String,
        object: String,
        #[arg(long, default_value = "GET", value_parser = ["GET", "PUT", "HEAD", "POST", "DELETE"])]
        method: String,
        #[arg(long, default_value_t = 3600, value_parser = clap::value_parser!(u64).range(1..))]
        expires: u64,
        #[arg(long)]
        cdn: bool,
        #[command(flatten)]
        metadata: MetadataArgs,
    },
    SetAcl {
        bucket: String,
        object: String,
        #[arg(long)]
        file: String,
        #[arg(long, value_enum, default_value = "add")]
        action: AclMode,
        #[arg(long)]
        version_id: Option<String>,
    },
    Exists {
        bucket: String,
        object: String,
    },
    Copy {
        src_bucket: String,
        src_object: String,
        dst_bucket: String,
        dst_object: String,
    },
    Rename {
        bucket: String,
        src: String,
        dst: String,
    },
    Delete {
        bucket: String,
        object: String,
    },
    DeleteMany {
        bucket: String,
        #[arg(required = true)]
        names: Vec<String>,
        #[arg(long)]
        trash: bool,
    },
    DeletePrefix {
        bucket: String,
        prefix: String,
        #[arg(long)]
        trash: bool,
        #[arg(long)]
        delimiter: Option<String>,
        #[arg(long)]
        marker: Option<String>,
        #[arg(long, default_value_t = 1000)]
        max_keys: i32,
    },
    List {
        bucket: String,
        #[arg(long, default_value = "")]
        prefix: String,
        #[arg(long, default_value = "")]
        delimiter: String,
        #[arg(long, default_value_t = 1000)]
        max_keys: i32,
        #[arg(long)]
        marker: Option<String>,
        /// Paginate through all batches
        #[arg(long)]
        all: bool,
    },
    /// Restore a deleted object (not archive thawing)
    Restore {
        bucket: String,
        object: String,
    },
    BasicInfo {
        bucket: String,
        object: String,
    },
    GetMetadata {
        bucket: String,
        object: String,
    },
    SetMetadata {
        bucket: String,
        object: String,
        #[command(flatten)]
        metadata: MetadataArgs,
    },
    GetAcl {
        bucket: String,
        object: String,
        #[arg(long)]
        version_id: Option<String>,
    },
    SetPublic {
        bucket: String,
        object: String,
    },
}

pub async fn run_object(client: &Client, cmd: &ObjectCmd) -> Result<()> {
    match cmd {
        ObjectCmd::Put {
            bucket,
            object,
            body,
            metadata,
            expires,
            expect,
            content_disposition,
        } => {
            let mut request = PutObjectRequest::new(bucket, object, body.load().await?)
                .metadata(metadata.build()?);
            if let Some(value) = expires {
                request = request.expires(*value);
            }
            if let Some(value) = expect {
                request = request.expect(value);
            }
            if let Some(value) = content_disposition {
                request = request.content_disposition(value);
            }
            print_json(&output::put(&request.send(client).await?))?;
        }
        ObjectCmd::Head { bucket, object } => match client.head_object(bucket, object).await? {
            Some(md) => print_json(md.as_map())?,
            None => println!("null"),
        },
        ObjectCmd::Prefetch { bucket, object } => {
            client.prefetch_object(bucket, object).await?;
            print_json(&serde_json::json!({"acknowledged":true,"completion_confirmed":false}))?;
        }
        ObjectCmd::Refresh { bucket, object } => {
            client.refresh_object(bucket, object).await?;
            print_json(&serde_json::json!({"acknowledged":true,"completion_confirmed":false}))?;
        }
        ObjectCmd::Url { bucket, object } => {
            println!("{}", client.generate_absolute_object_url(bucket, object)?)
        }
        ObjectCmd::Presign {
            bucket,
            object,
            method,
            expires,
            cdn,
            metadata,
        } => {
            let duration = std::time::Duration::from_secs(*expires);
            let request = GeneratePresignedURLRequest::expires_in(
                bucket,
                object,
                parse_method(method)?,
                duration,
            )?
            .cdn(*cdn)
            .metadata(metadata.build()?);
            println!("{}", request.generate(client)?);
        }
        ObjectCmd::SetAcl {
            bucket,
            object,
            file,
            action,
            version_id,
        } => {
            let mut request =
                SetObjectACLRequest::new(bucket, object, read_json(file)?).action(action.sdk());
            if let Some(version) = version_id {
                request = request.version_id(version);
            }
            request.send(client).await?;
            output::ok()?;
        }
        ObjectCmd::Get {
            bucket,
            object,
            range,
            out,
            show_headers,
        } => {
            let output = client
                .get_object(&GetObjectRequest {
                    bucket_name: bucket.clone(),
                    object_name: object.clone(),
                    range: range.as_deref().map(str::parse).transpose()?,
                    timeout: None,
                })
                .await?;
            if *show_headers {
                let headers: Vec<_> = output
                    .headers()
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str(),
                            String::from_utf8_lossy(value.as_bytes()).into_owned(),
                        )
                    })
                    .collect();
                eprintln!(
                    "{}",
                    serde_json::json!({"status": output.status(), "headers": headers})
                );
            }
            match out {
                Some(path) => super::util::save_download(output, path).await?,
                None => {
                    output.copy_to(&mut tokio::io::stdout()).await?;
                }
            }
        }

        ObjectCmd::Exists { bucket, object } => {
            println!("{}", client.does_object_exist(bucket, object).await?)
        }
        ObjectCmd::Copy {
            src_bucket,
            src_object,
            dst_bucket,
            dst_object,
        } => {
            client
                .copy_object(&CopyObjectRequest {
                    source_bucket_name: src_bucket.clone(),
                    source_object_name: src_object.clone(),
                    target_bucket_name: dst_bucket.clone(),
                    target_object_name: dst_object.clone(),
                })
                .await?;
            output::ok()?;
        }
        ObjectCmd::Rename { bucket, src, dst } => {
            client.rename_object(bucket, src, dst).await?;
            output::ok()?;
        }
        ObjectCmd::Delete { bucket, object } => {
            client.delete_object(bucket, object).await?;
            output::ok()?;
        }
        ObjectCmd::DeleteMany {
            bucket,
            names,
            trash,
        } => {
            output::delete_result(client.delete_objects(bucket, names, *trash).await, names)?;
        }
        ObjectCmd::DeletePrefix {
            bucket,
            prefix,
            trash,
            delimiter,
            marker,
            max_keys,
        } => {
            output::delete_result(
                client
                    .delete_objects_with_prefix(
                        &ListObjectsRequest::new(bucket)
                            .prefix(prefix)
                            .delimiter(delimiter.clone().unwrap_or_default())
                            .marker(marker.clone().unwrap_or_default())
                            .max_keys(*max_keys),
                        *trash,
                    )
                    .await,
                &[],
            )?;
        }
        ObjectCmd::List {
            bucket,
            prefix,
            delimiter,
            max_keys,
            all,
            marker,
        } => {
            use futures_util::{pin_mut, TryStreamExt};
            let pages = ListObjectsRequest::new(bucket)
                .prefix(prefix)
                .delimiter(delimiter)
                .max_keys(*max_keys)
                .marker(marker.clone().unwrap_or_default())
                .pages(client);
            pin_mut!(pages);
            while let Some(listing) = pages.try_next().await? {
                print_json(&output::listing(&listing))?;
                if !*all {
                    break;
                }
            }
        }

        ObjectCmd::Restore { bucket, object } => {
            client.restore_object(bucket, object).await?;
            output::ok()?;
        }
        ObjectCmd::BasicInfo { bucket, object } => print_json(&output::basic(
            &client.get_object_basic_info(bucket, object).await?,
        ))?,
        ObjectCmd::GetMetadata { bucket, object } => {
            let md = client.get_object_metadata(bucket, object).await?;
            print_json(md.as_map())?;
        }
        ObjectCmd::SetMetadata {
            bucket,
            object,
            metadata,
        } => {
            client
                .set_object_metadata(&SetObjectMetadataRequest {
                    bucket_name: bucket.clone(),
                    object_name: object.clone(),
                    metadata: metadata.build()?,
                })
                .await?;
            output::ok()?;
        }
        ObjectCmd::GetAcl {
            bucket,
            object,
            version_id,
        } => {
            let acl = client
                .get_object_acl(&GetObjectACLRequest {
                    bucket_name: bucket.clone(),
                    object_name: object.clone(),
                    version_id: version_id.clone(),
                })
                .await?;
            print_json(&acl)?;
        }
        ObjectCmd::SetPublic { bucket, object } => {
            client.set_object_public(bucket, object).await?;
            output::ok()?;
        }
    }
    Ok(())
}
