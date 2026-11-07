use anyhow::anyhow;
use anyhow::{bail, Result};
use fds::{acl::*, object::*};
use http::Method;
pub fn parse_method(s: &str) -> Result<Method> {
    Ok(match s.to_ascii_uppercase().as_str() {
        "GET" => Method::GET,
        "PUT" => Method::PUT,
        "HEAD" => Method::HEAD,
        "POST" => Method::POST,
        "DELETE" => Method::DELETE,
        other => bail!("unknown method {other}"),
    })
}

/// Parse repeated `key=value` into an `ObjectMetadata`.
pub fn parse_meta(pairs: &[String]) -> Result<ObjectMetadata> {
    let mut md = ObjectMetadata::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| anyhow!("bad --meta {p:?} (expected key=value)"))?;
        md.set(k, v)?;
    }
    Ok(md)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: impl AsRef<std::path::Path>) -> Result<T> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
pub fn print_json(value: &impl serde::Serialize) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum AclMode {
    Add,
    Delete,
}
impl AclMode {
    pub fn sdk(self) -> AclAction {
        match self {
            Self::Add => AclAction::Add,
            Self::Delete => AclAction::Delete,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum CannedAcl {
    PublicRead,
    PublicWrite,
    PublicReadObjects,
    PublicSsoWrite,
    AuthenticatedRead,
    AuthenticatedWrite,
    AuthenticatedSsoWrite,
}
impl CannedAcl {
    pub fn sdk(self) -> BucketCannedAcl {
        match self {
            Self::PublicRead => BucketCannedAcl::PublicRead,
            Self::PublicWrite => BucketCannedAcl::PublicWrite,
            Self::PublicReadObjects => BucketCannedAcl::PublicReadObjects,
            Self::PublicSsoWrite => BucketCannedAcl::PublicSsoWrite,
            Self::AuthenticatedRead => BucketCannedAcl::AuthenticatedRead,
            Self::AuthenticatedWrite => BucketCannedAcl::AuthenticatedWrite,
            Self::AuthenticatedSsoWrite => BucketCannedAcl::AuthenticatedSsoWrite,
        }
    }
}

#[derive(clap::Args)]
#[group(skip)]
#[command(group(clap::ArgGroup::new("body").required(true).args(["file", "text", "stdin"])))]
pub struct BodyArgs {
    /// Stream a regular file without buffering the whole object
    #[arg(long)]
    file: Option<std::path::PathBuf>,
    /// In-memory UTF-8 payload (can be empty)
    #[arg(long)]
    text: Option<String>,
    /// Stream stdin; requires an exact --length
    #[arg(long, requires = "length")]
    stdin: bool,
    /// Declared bytes to send from stdin; trailing bytes are not consumed
    #[arg(long, requires = "stdin", conflicts_with_all = ["file", "text"])]
    length: Option<u64>,
}
impl BodyArgs {
    pub async fn load(&self) -> Result<UploadBody> {
        match (&self.file, &self.text, self.stdin) {
            (Some(path), None, false) => Ok(UploadBody::from_file(path).await?),
            (None, Some(text), false) => Ok(text.as_bytes().to_vec().into()),
            (None, None, true) => Ok(UploadBody::from_stream(
                tokio_util::io::ReaderStream::with_capacity(tokio::io::stdin(), 64 * 1024),
                self.length
                    .ok_or_else(|| anyhow!("stdin requires --length"))?,
            )),
            _ => bail!("choose exactly one of --file, --text, --stdin"),
        }
    }
}

#[derive(clap::Args, Default)]
pub struct MetadataArgs {
    /// Repeatable SDK metadata entries, e.g. --meta x-xiaomi-meta-owner=me
    #[arg(long = "meta")]
    pub meta: Vec<String>,
    #[arg(long)]
    content_type: Option<String>,
    #[arg(long)]
    cache_control: Option<String>,
    #[arg(long)]
    content_encoding: Option<String>,
    #[arg(long)]
    content_md5: Option<String>,
}
impl MetadataArgs {
    pub fn build(&self) -> Result<ObjectMetadata> {
        let mut md = parse_meta(&self.meta)?;
        for (key, value) in [
            ("content-type", &self.content_type),
            ("cache-control", &self.cache_control),
            ("content-encoding", &self.content_encoding),
            ("content-md5", &self.content_md5),
        ] {
            if let Some(value) = value {
                if md.get(key).is_some() {
                    bail!("{key} supplied both as a flag and --meta");
                }
                md.set(key, value)?;
            }
        }
        Ok(md)
    }
}

/// Stage in the destination directory. A failed stream never truncates an existing destination.
pub async fn save_download(output: GetObjectOutput, path: &str) -> Result<()> {
    let target = std::path::Path::new(path);
    let mut staging = target.as_os_str().to_os_string();
    staging.push(format!(".fds-{}.tmp", std::process::id()));
    let staging = std::path::PathBuf::from(staging);
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .await?;
    let result = async {
        use tokio::io::AsyncWriteExt;
        let bytes = output.copy_to(&mut file).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&staging, target).await?;
        eprintln!("downloaded {bytes} bytes to {path}");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&staging).await;
    }
    result
}
