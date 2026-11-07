//! # fds — async Rust SDK for XiaoMi FDS object storage
//!
//! Build a validated [`Client`], construct operation-specific requests, and
//! consume streaming responses with Tokio. Galaxy-V2 signing is protected by
//! saved golden vectors and local wire tests; these do not prove live-service
//! compatibility for every operation. OAuth exchange is explicit.
//!
//! ```no_run
//! # async fn example(client: &fds::Client) -> fds::Result<()> {
//! fds::PutObjectRequest::new("bucket", "hello.txt", b"hello".to_vec())
//!     .content_type("text/plain").send(client).await?;
//! let mut output = fds::GetObjectRequest::new("bucket", "hello.txt")
//!     .send(client).await?;
//! while let Some(chunk) = output.chunk().await? {
//!     // Consume each chunk without collecting the entire object.
//!     let _ = chunk;
//! }
//! # Ok(()) }
//! ```
//!
//! Every fallible call returns [`Result`]; failures are a single opaque
//! [`Error`], classified by [`ErrorKind`] and [`Dispatch`]. See [`error`].
//!
//! POST object creation, archive restoration and multipart copy remain deferred.
//! See the repository's `docs/migration.md` for compatibility and validation.

pub mod acl;
pub mod auth;
pub mod bucket;
pub mod client;
pub mod config;
pub mod constant;
#[cfg(feature = "cslb")]
pub mod cslb;
pub(crate) mod de;
pub mod download;
pub mod encoding;
pub mod error;
#[cfg(feature = "manager")]
pub mod manager;
pub mod metadata;
pub mod multipart;
pub mod oauth;
pub mod object;
pub mod retry;
pub mod upload;

pub use acl::{
    AccessControlList, AclAction, BucketCannedAcl, Grant, GrantKey, GrantPermission, GrantType,
    Owner,
};
pub use bucket::{
    AccessLog, CreateBucketRequest, GetBucketInfoResponse, GetLifecycleConfigRequest,
    LifecycleBaseItem, LifecycleConfig, LifecycleRule, ListBucketsResponse, MigrateBucketRequest,
    SetBucketAclRequest,
};
pub use client::Client;
pub use config::{ClientBuilder, ClientConfiguration};
pub use constant::{HttpMethod, StorageClass};
pub use download::{ByteRange, GetObjectOutput};
pub use error::{Dispatch, Error, ErrorKind, ResponseDetails, Result};
#[cfg(feature = "manager")]
pub use manager::{DownloadOptions, DownloadOptionsBuilder, DownloadRequest, Downloader};
pub use metadata::ObjectMetadata;
pub use object::{
    CompleteMultipartUploadRequest, CopyObjectRequest, CopyObjectResponse, DeleteObjectFailure,
    DeleteObjectsOutput, GeneratePresignedURLRequest, GetObjectACLRequest, GetObjectRequest,
    InitMultipartUploadRequest, InitMultipartUploadResponse, ListObjectsRequest, ObjectBasicInfo,
    ObjectListing, ObjectSummary, PutObjectRequest, PutObjectResponse, SetObjectACLRequest,
    SetObjectMetadataRequest, UploadPartRequest, UploadPartResponse,
};

pub use upload::UploadBody;

pub use retry::RetryPolicy;

pub use multipart::{
    CancellationToken, CleanupPolicy, MultipartOptions, MultipartProgress, MultipartState,
    MultipartUpload,
};

pub use oauth::{
    AppBucket, AppBuckets, FdsServiceClient, OAuthProvider, StorageAccessToken, StorageTokenRequest,
};
