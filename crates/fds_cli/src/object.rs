use super::output;
use super::util::{
    apply_metadata, parse_method, presign_metadata, print_json, read_json, AclMode, BodyArgs,
    MetadataArgs,
};
use anyhow::Result;
use clap::Subcommand;
use fds::*;
use std::collections::BTreeMap;
use std::num::NonZeroU32;
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
            let mut request = apply_metadata!(
                client.put_object(bucket, object, body.load().await?),
                metadata.build()?
            );
            if let Some(value) = expires {
                request = request.expires(*value);
            }
            if let Some(value) = expect {
                request = request.expect(value);
            }
            if let Some(value) = content_disposition {
                request = request.content_disposition(value);
            }
            print_json(&output::put(&request.send().await?))?;
        }
        ObjectCmd::Head { bucket, object } => {
            match client.head_object(bucket, object).send().await? {
                Some(md) => print_json(&md.entries().collect::<BTreeMap<_, _>>())?,
                None => println!("null"),
            }
        }
        ObjectCmd::Prefetch { bucket, object } => {
            client.prefetch_object(bucket, object).send().await?;
            print_json(&serde_json::json!({"acknowledged":true,"completion_confirmed":false}))?;
        }
        ObjectCmd::Refresh { bucket, object } => {
            client.refresh_object(bucket, object).send().await?;
            print_json(&serde_json::json!({"acknowledged":true,"completion_confirmed":false}))?;
        }
        ObjectCmd::Url { bucket, object } => {
            println!("{}", client.absolute_object_url(bucket, object)?)
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
            let request = client
                .presign_object(bucket, object)
                .method(parse_method(method)?)
                .expires_in(duration);
            let mut request = presign_metadata(request, metadata.build()?);
            if *cdn {
                request = request.cdn();
            }
            println!("{}", request.build()?);
        }
        ObjectCmd::SetAcl {
            bucket,
            object,
            file,
            action,
            version_id,
        } => {
            let mut request = client
                .set_object_acl(bucket, object, read_json(file)?)
                .action(action.sdk());
            if let Some(version) = version_id {
                request = request.version_id(version);
            }
            request.send().await?;
            output::ok()?;
        }
        ObjectCmd::Get {
            bucket,
            object,
            range,
            out,
            show_headers,
        } => {
            let mut request = client.get_object(bucket, object);
            if let Some(range) = range.as_deref().map(str::parse).transpose()? {
                request = request.range(range);
            }
            let output = request.send().await?;
            if *show_headers {
                let headers: Vec<_> = output
                    .headers()
                    .iter()
                    .map(|(name, value)| (name, String::from_utf8_lossy(value).into_owned()))
                    .collect();
                eprintln!(
                    "{}",
                    serde_json::json!({"status": output.status().as_u16(), "headers": headers})
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
            println!("{}", client.object_exists(bucket, object).send().await?)
        }
        ObjectCmd::Copy {
            src_bucket,
            src_object,
            dst_bucket,
            dst_object,
        } => {
            client
                .copy_object((src_bucket, src_object), (dst_bucket, dst_object))
                .send()
                .await?;
            output::ok()?;
        }
        ObjectCmd::Rename { bucket, src, dst } => {
            client.rename_object(bucket, src, dst).send().await?;
            output::ok()?;
        }
        ObjectCmd::Delete { bucket, object } => {
            client.delete_object(bucket, object).send().await?;
            output::ok()?;
        }
        ObjectCmd::DeleteMany {
            bucket,
            names,
            trash,
        } => {
            let mut request = client.delete_objects(bucket).keys(names);
            if *trash {
                request = request.move_to_trash();
            }
            output::delete_result(request.send().await, names)?;
        }
        ObjectCmd::DeletePrefix {
            bucket,
            prefix,
            trash,
            delimiter,
            marker,
            max_keys,
        } => {
            let page_size = match positive_max_keys(*max_keys) {
                Ok(page_size) => page_size,
                Err(error) => {
                    return output::delete_outcome(Default::default(), Some(error), &[]);
                }
            };
            // The marker is always sent, empty when `--marker` is not given.
            let mut request = client
                .delete_prefix(bucket, prefix)
                .delimiter(delimiter.as_deref().unwrap_or_default())
                .marker(marker.as_deref().unwrap_or_default())
                .page_size(page_size);
            if *trash {
                request = request.move_to_trash();
            }
            match request.send().await {
                Ok(deleted) => output::delete_outcome(deleted, None, &[])?,
                Err(error) => {
                    let deleted = error.partial_delete_output().cloned().unwrap_or_default();
                    output::delete_outcome(deleted, Some(page_check_error(error)), &[])?;
                }
            }
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
            let pages = client
                .list_objects(bucket)
                .prefix(prefix)
                .delimiter(delimiter)
                .max_keys(positive_max_keys(*max_keys)?)
                .marker(marker.clone().unwrap_or_default())
                .into_pages();
            pin_mut!(pages);
            while let Some(listing) = pages.try_next().await? {
                print_json(&output::listing(&listing))?;
                if !*all {
                    break;
                }
            }
        }

        ObjectCmd::Restore { bucket, object } => {
            client.restore_object(bucket, object).send().await?;
            output::ok()?;
        }
        ObjectCmd::BasicInfo { bucket, object } => print_json(&output::basic(
            &client.get_object_basic_info(bucket, object).send().await?,
        ))?,
        ObjectCmd::GetMetadata { bucket, object } => {
            let md = client.get_object_metadata(bucket, object).send().await?;
            print_json(&md.entries().collect::<BTreeMap<_, _>>())?;
        }
        ObjectCmd::SetMetadata {
            bucket,
            object,
            metadata,
        } => {
            apply_metadata!(
                client.set_object_metadata(bucket, object),
                metadata.build()?
            )
            .send()
            .await?;
            output::ok()?;
        }
        ObjectCmd::GetAcl {
            bucket,
            object,
            version_id,
        } => {
            let mut request = client.get_object_acl(bucket, object);
            if let Some(version) = version_id {
                request = request.version_id(version);
            }
            let acl = request.send().await?;
            print_json(&acl)?;
        }
        ObjectCmd::SetPublic { bucket, object } => {
            client.set_object_public(bucket, object).send().await?;
            output::ok()?;
        }
    }
    Ok(())
}

/// `--max-keys` keeps its `i32` command-line type; zero and negative values
/// fail like the SDK's former runtime check did.
fn positive_max_keys(max_keys: i32) -> fds::Result<NonZeroU32> {
    u32::try_from(max_keys)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| fds::Error::new(ErrorKind::InvalidInput, "max_keys must be positive"))
}

/// `object delete-prefix` reports both of the SDK's page checks (a listed name
/// outside the prefix, a missing or repeated `nextMarker`) with the command's
/// original single message. They are the only `ProtocolViolation`s of the run
/// without a response: the client's own ones (an empty, oversized or invalid
/// body) carry it, and a bulk delete's become `OutcomeUnknown`.
fn page_check_error(error: fds::Error) -> fds::Error {
    if error.kind() == ErrorKind::ProtocolViolation && error.response().is_none() {
        fds::Error::new(
            ErrorKind::ProtocolViolation,
            "invalid prefix deletion page or pagination marker",
        )
    } else {
        error
    }
}
