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

/// The metadata of a write command: the typed system headers and the
/// `x-xiaomi-meta-*` user entries.
#[derive(Default)]
pub struct Metadata {
    pub(crate) content_type: Option<String>,
    pub(crate) cache_control: Option<String>,
    pub(crate) content_encoding: Option<String>,
    pub(crate) content_md5: Option<String>,
    pub(crate) user: UserMetadata,
}

const META_PREFIX: &str = "x-xiaomi-meta-";

impl Metadata {
    /// The typed slot for a lowercase header name.
    fn slot(&mut self, key: &str) -> Option<&mut Option<String>> {
        match key {
            "content-type" => Some(&mut self.content_type),
            "cache-control" => Some(&mut self.cache_control),
            "content-encoding" => Some(&mut self.content_encoding),
            "content-md5" => Some(&mut self.content_md5),
            _ => None,
        }
    }

    /// Set one `--meta` entry. Keys ignore case; a later entry replaces an
    /// earlier one. Only the typed headers and `x-xiaomi-meta-*` are accepted.
    fn set(&mut self, k: &str, v: &str) -> Result<()> {
        let key = k.to_ascii_lowercase();
        if let Some(slot) = self.slot(&key) {
            *slot = Some(v.to_string());
        } else if let Some(user) = key.strip_prefix(META_PREFIX) {
            self.user.insert(user, v);
        } else {
            return Err(fds::Error::new(
                fds::ErrorKind::InvalidInput,
                format!("Invalid metadata: {k}"),
            )
            .into());
        }
        Ok(())
    }

    fn contains(&mut self, key: &str) -> bool {
        self.slot(key).is_some_and(|slot| slot.is_some())
    }

    /// The user entries, for builders that take only those.
    pub fn into_user(self) -> UserMetadata {
        self.user
    }
}

/// Apply a [`Metadata`] to a builder with the SDK's typed metadata setters.
macro_rules! apply_metadata {
    ($request:expr, $metadata:expr) => {{
        let $crate::util::Metadata {
            content_type,
            cache_control,
            content_encoding,
            content_md5,
            user,
        } = $metadata;
        let mut request = $request;
        if let Some(value) = content_type {
            request = request.content_type(value);
        }
        if let Some(value) = cache_control {
            request = request.cache_control(value);
        }
        if let Some(value) = content_encoding {
            request = request.content_encoding(value);
        }
        if let Some(value) = content_md5 {
            request = request.content_md5(value);
        }
        request.user_metadata(user)
    }};
}
pub(crate) use apply_metadata;

/// Apply a [`Metadata`] to a presigned URL: only `Content-Type`,
/// `Content-MD5` and the user entries are signed, so the other headers do
/// not change the URL.
pub fn presign_metadata(mut request: PresignObject, metadata: Metadata) -> PresignObject {
    if let Some(value) = metadata.content_type {
        request = request.content_type(value);
    }
    if let Some(value) = metadata.content_md5 {
        request = request.content_md5(value);
    }
    request.user_metadata(metadata.user)
}

/// Parse repeated `key=value` into a [`Metadata`].
pub fn parse_meta(pairs: &[String]) -> Result<Metadata> {
    let mut md = Metadata::default();
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
/// Read a lifecycle JSON file. A decode error reports only the parser
/// position (serde's message can echo input values), as an SDK invalid-input
/// error, exactly as the SDK's former `from_json` helpers did.
pub fn read_json_redacted<T: serde::de::DeserializeOwned>(
    path: impl AsRef<std::path::Path>,
) -> Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        fds::Error::new(fds::ErrorKind::InvalidInput, "invalid JSON input")
            .with_context("category", format_args!("{:?}", error.classify()))
            .with_context("line", error.line())
            .with_context("column", error.column())
            .into()
    })
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
    pub fn build(&self) -> Result<Metadata> {
        let mut md = parse_meta(&self.meta)?;
        for (key, value) in [
            ("content-type", &self.content_type),
            ("cache-control", &self.cache_control),
            ("content-encoding", &self.content_encoding),
            ("content-md5", &self.content_md5),
        ] {
            if let Some(value) = value {
                if md.contains(key) {
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
