//! # fds — async Rust SDK for XiaoMi FDS object storage
//!
//! Build a validated [`Client`], construct operation-specific requests, and
//! consume streaming responses with Tokio. Galaxy-V2 signing is protected by
//! saved golden vectors and local wire tests; these do not prove live-service
//! compatibility for every operation. OAuth exchange is explicit.
//!
//! ```no_run
//! use fds::object::{GetObjectRequest, PutObjectRequest};
//!
//! # async fn example(client: &fds::Client) -> fds::Result<()> {
//! PutObjectRequest::new("bucket", "hello.txt", b"hello".to_vec())
//!     .content_type("text/plain").send(client).await?;
//! let mut output = GetObjectRequest::new("bucket", "hello.txt")
//!     .send(client).await?;
//! while let Some(chunk) = output.chunk().await? {
//!     // Consume each chunk without collecting the entire object.
//!     let _ = chunk;
//! }
//! # Ok(()) }
//! ```
//!
//! Every fallible call returns [`Result`]; failures are a single opaque
//! [`Error`], classified by [`ErrorKind`] and [`Dispatch`].
//!
//! # Module layout
//!
//! **Every public type has exactly one import path.**
//!
//! - The crate root exports only the client and error core: [`Client`],
//!   [`ClientBuilder`], [`ClientConfiguration`], [`RetryPolicy`], [`Error`],
//!   [`ErrorKind`], [`Dispatch`], [`ResponseDetails`] and [`Result`].
//! - Operation inputs, outputs and models live in exactly one public domain
//!   module, chosen by the resource they act on:
//!   - [`object`]: single-object operations, listing, bulk delete, presigned
//!     URLs and the object models ([`object::ObjectMetadata`],
//!     [`object::UploadBody`], [`object::GetObjectOutput`],
//!     [`object::ByteRange`], [`object::StorageClass`]);
//!   - [`bucket`]: bucket operations, lifecycle and access-log models;
//!   - [`acl`]: ACL models shared by buckets and objects;
//!   - [`multipart`]: the low-level multipart requests and the managed
//!     [`multipart::MultipartUpload`];
//!   - [`oauth`]: endpoint discovery and storage-token exchange;
//!   - `manager` (default `manager` feature): the resumable `Downloader`.
//! - A type defined in a domain module is never re-exported from the root or
//!   from another module, and implementation modules are private. New
//!   operations and their builders follow the same rule.
//! - `fds` does not re-export third-party crates. `http` types such as
//!   [`http::Method`] and [`http::StatusCode`] appear directly in signatures;
//!   cancellation signals are plain `Future<Output = ()>` values.
//! - The `cslb` feature adds no module: it only makes [`Client`] resolve hosts
//!   through an internal client-side load-balancing DNS resolver, configured
//!   by `ClientBuilder::cslb_max_nodes`.
//!
//! POST object creation, archive restoration and multipart copy remain deferred.
//! See the repository's `docs/migration.md` for compatibility and validation.

mod auth;
mod client;
mod config;
mod constant;
#[cfg(feature = "cslb")]
mod cslb;
mod de;
mod download;
mod encoding;
mod error;
mod idle;
mod metadata;
mod retry;
mod upload;

pub mod acl;
pub mod bucket;
#[cfg(feature = "manager")]
pub mod manager;
pub mod multipart;
pub mod oauth;
pub mod object;

pub use client::Client;
pub use config::{ClientBuilder, ClientConfiguration};
pub use error::{Dispatch, Error, ErrorKind, ResponseDetails, Result};
pub use retry::RetryPolicy;
