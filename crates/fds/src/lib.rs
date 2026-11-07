//! # fds — async Rust SDK for XiaoMi FDS object storage
//!
//! Build a validated [`Client`], start an operation from it, and consume
//! streaming responses with Tokio. Galaxy-V2 signing is protected by saved
//! golden vectors and local wire tests; these do not prove live-service
//! compatibility for every operation. OAuth exchange is explicit.
//!
//! ```no_run
//! # async fn example(client: &fds::Client) -> fds::Result<()> {
//! client
//!     .put_object("bucket", "hello.txt", b"hello".to_vec())
//!     .content_type("text/plain")
//!     .send()
//!     .await?;
//! let mut output = client.get_object("bucket", "hello.txt").send().await?;
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
//! # Operation builders
//!
//! Every operation is written as
//! `client.<operation>(required arguments).<option>(..).send().await`.
//! Single-object operations follow this convention today; bucket, listing,
//! bulk-delete and multipart operations still use request structs until they
//! are migrated to it.
//!
//! - The [`Client`] method takes only the required arguments (bucket, object,
//!   body, ACL, ...) and returns a builder type named after the operation,
//!   defined in the operation's domain module (for example
//!   [`object::GetObject`] for [`Client::get_object`]). Bucket and object
//!   names are taken as `impl Into<String>`.
//! - The builder owns a clone of the [`Client`] (cheap: clones share one
//!   connection pool) and its arguments. Its fields are private and it does
//!   not implement `Default`, so it can only be created from the [`Client`]
//!   method, with every required argument present.
//! - Optional settings are chained methods that take and return the builder
//!   by value. A flag is a method without arguments (for example
//!   [`object::PresignObject::cdn`]), never a `bool` parameter.
//! - `send(self)` consumes the builder and returns a future. All local
//!   validation (bucket and object names, header values, the timeout) happens
//!   there, before anything is sent, and fails with
//!   [`ErrorKind::InvalidInput`] and [`Dispatch::NotSent`].
//! - Every builder that sends a request has `timeout(Duration)`, a total
//!   deadline for that request overriding
//!   [`ClientBuilder::request_timeout`].
//! - An operation that makes no request ends in `build()` instead of `send()`
//!   and has no timeout (for example [`Client::presign_object`]); one without
//!   options is a plain method (for example [`Client::absolute_object_url`]).
//! - Builders are `#[must_use]`: nothing happens until `send` is awaited.
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
