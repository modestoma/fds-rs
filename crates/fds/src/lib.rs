//! # fds — async Rust SDK for FDS (File Data Storage) object storage
//!
//! Build a validated [`Client`], start an operation from it, and consume
//! streaming responses with Tokio. Galaxy-V2 signing is protected by saved
//! golden vectors and local wire tests; these do not prove live-service
//! compatibility for every operation.
//!
//! # Quick start
//!
//! ```no_run
//! use fds::{Client, ErrorKind};
//!
//! # async fn example() -> fds::Result<()> {
//! // The SDK reads no environment variables; the application supplies them.
//! let client = Client::builder()
//!     .endpoint("https://fds.example.com") // replace with your endpoint
//!     .credentials("ACCESS_KEY_ID", "ACCESS_KEY_SECRET")
//!     .build()?;
//!
//! client
//!     .put_object("bucket", "hello.txt", b"hello".to_vec())
//!     .content_type("text/plain")
//!     .send()
//!     .await?;
//!
//! let mut output = client.get_object("bucket", "hello.txt").send().await?;
//! println!("{:?} bytes, etag {:?}", output.content_length(), output.etag());
//! while let Some(chunk) = output.chunk().await? {
//!     // Consume each chunk without collecting the entire object.
//!     let _ = chunk;
//! }
//!
//! match client.get_object("bucket", "missing.txt").send().await {
//!     Ok(output) => println!("status {}", output.status()),
//!     Err(error) if error.is_not_found() => println!("no such object"),
//!     Err(error) if error.kind() == ErrorKind::PermissionDenied => return Err(error),
//!     Err(error) => return Err(error),
//! }
//! # Ok(()) }
//! ```
//!
//! A [`Client`] is cheap to clone: clones share one connection pool, the
//! configuration and the credentials provider. Build it once and reuse it.
//!
//! # Errors
//!
//! Every fallible call returns [`Result`]; every failure is one opaque
//! [`Error`]:
//!
//! - [`Error::kind`] classifies it by [`ErrorKind`] (`NotFound`,
//!   `PermissionDenied`, `Unauthenticated`, `RateLimited`, `ServerError`,
//!   `InvalidInput`, `ProtocolViolation`, `Timeout`, `Transport`, `Io`,
//!   `OutcomeUnknown`, ...). `ErrorKind` is `#[non_exhaustive]`.
//! - [`Error::dispatch`] tells whether the request reached the service
//!   ([`Dispatch::NotSent`], [`Dispatch::Sent`] or [`Dispatch::Unknown`]).
//! - [`Error::is_not_found`], [`Error::is_retryable`] and
//!   [`Error::is_outcome_unknown`] are shortcuts; `OutcomeUnknown` means a
//!   write (copy, multipart completion, bulk deletion) may have taken effect
//!   and must not be repeated blindly.
//! - [`Error::response`] exposes the HTTP status and bounded service
//!   diagnostics ([`ResponseDetails`]); [`Error::partial_delete_output`]
//!   carries the confirmed progress of a failed bulk or prefix deletion.
//!
//! `Display` and `Debug` never print access keys, signatures, storage tokens,
//! credential-bearing URLs, request IDs or raw response bodies.
//!
//! # Timeouts, retries and credentials
//!
//! - There is no total deadline by default. Every request is bounded by
//!   [`ClientBuilder::connect_timeout`] and by
//!   [`ClientBuilder::read_timeout`], an I/O idle timeout that progress
//!   resets: each uploaded body chunk, and each received response chunk.
//!   A transfer that keeps moving never times out; one that stalls fails
//!   with [`ErrorKind::Timeout`]. [`ClientBuilder::request_timeout`] and the
//!   builders' `timeout(Duration)` opt in to a total deadline.
//! - Retries are off by default. A [`RetryPolicy`] retries reads;
//!   [`RetryPolicy::retry_idempotent_writes`] or a request's
//!   `retry(RetryMode::Enabled)` also retries idempotent writes with an
//!   in-memory body. Non-idempotent writes are never retried.
//! - The client asks its [`credentials::CredentialsProvider`] before every
//!   request attempt. [`credentials::StorageTokenProvider`] exchanges an
//!   OAuth storage token again shortly before it expires, without rebuilding
//!   the client. Known boundary: a token seeded with
//!   [`credentials::StorageTokenProvider::with_token`] has an unknown
//!   lifetime, so the full refresh margin applies to it and the half-lifetime
//!   cap used for tokens the provider exchanged itself does not; a seeded
//!   token that expires within the margin is exchanged again on the first
//!   request.
//!
//! # Features
//!
//! - `manager` (default): `Client::download_file`, the concurrent,
//!   resumable file download, and its `manager::RequestLimiter`. The managed
//!   multipart *upload* ([`Client::upload_file`]) is part of the core SDK.
//! - `cslb` (off by default): [`Client`] resolves hosts through an internal
//!   client-side load-balancing DNS resolver. It adds no public module; its
//!   only setting is `ClientBuilder::cslb_max_nodes`.
//!
//! Every feature combination builds and is tested. The minimum supported
//! Rust version is 1.86.
//!
//! # Module layout
//!
//! **Every public type has exactly one import path.**
//!
//! - The crate root exports only the client and error core: [`Client`],
//!   [`ClientBuilder`], [`ClientConfiguration`], [`RetryPolicy`],
//!   [`RetryMode`], [`Error`], [`ErrorKind`], [`Dispatch`],
//!   [`ResponseDetails`] and [`Result`].
//! - Operation inputs, outputs and models live in exactly one public domain
//!   module, chosen by the resource they act on:
//!   - [`object`]: single-object operations, listing, bulk delete, presigned
//!     URLs and the object models ([`object::ObjectMetadata`],
//!     [`object::ObjectAttributes`], [`object::UserMetadata`],
//!     [`object::UploadBody`], [`object::GetObjectOutput`],
//!     [`object::ResponseHeaders`],
//!     [`object::ByteRange`], [`object::ContentRange`],
//!     [`object::StorageClass`]);
//!   - [`bucket`]: bucket operations, lifecycle and access-log models;
//!   - [`acl`]: ACL models shared by buckets and objects;
//!   - [`credentials`]: [`credentials::Credentials`], the
//!     [`credentials::CredentialsProvider`] trait a client asks before every
//!     request attempt, and its built-in providers
//!     [`credentials::StaticCredentials`] and the refreshing
//!     [`credentials::StorageTokenProvider`];
//!   - [`multipart`]: the low-level multipart builders, the
//!     [`multipart::MultipartUploadSession`] and the managed file upload
//!     [`multipart::MultipartUpload`];
//!   - [`oauth`]: endpoint discovery, storage-token exchange and the
//!     app-scoped bucket listing;
//!   - `manager` (default `manager` feature): the resumable file download
//!     `DownloadFile` and its `RequestLimiter`.
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
//! Every operation follows this convention: single-object, bucket, OAuth,
//! listing, bulk-delete, multipart and managed transfer operations.
//!
//! - The [`Client`] method takes only the required arguments (bucket, object,
//!   body, ACL, ...) and returns a builder type named after the operation,
//!   defined in the operation's domain module (for example
//!   [`object::GetObject`] for [`Client::get_object`]). Bucket and object
//!   names are taken as `impl Into<String>`. Discovery and token exchange
//!   start from [`oauth::FdsServiceClient`] the same way.
//! - The builder owns a clone of the [`Client`] (cheap: clones share one
//!   connection pool) and its arguments. Its fields are private and it does
//!   not implement `Default`, so it can only be created from the [`Client`]
//!   method, with every required argument present.
//! - Optional settings are chained methods that take and return the builder
//!   by value. A flag is a method without arguments (for example
//!   [`object::PresignObject::cdn`]), never a `bool` parameter. An unset
//!   option is simply not called; no argument uses an empty string to mean
//!   "not set" (for example [`bucket::CreateBucket::org_id`]).
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
//! - Managed transfers ([`Client::upload_file`], `Client::download_file`)
//!   send many requests, so they have no single-request `timeout`; the
//!   client's connect and idle timeouts bound every request. Their options
//!   share names (`part_size`, `concurrency`) and are validated, like names,
//!   before any file or network IO, failing with [`ErrorKind::InvalidInput`].
//!   `upload_file` ends in `start()`, which initiates the upload and returns
//!   the [`multipart::MultipartUpload`] session to run, so that progress can
//!   be inspected and recovered after a failure.
//! - The builder of an idempotent write (for example
//!   [`object::PutObject`]) also has `retry(RetryMode)`, overriding the
//!   client's [`RetryPolicy`] for that request; builders of writes that are
//!   never retried do not have it. See [`RetryPolicy`] for the list.
//! - Builders are `#[must_use]`: nothing happens until `send` is awaited.
//!
//!
//! # Examples
//!
//! The repository's `crates/fds/examples` directory holds programs compiled
//! with every check: `quickstart` (put, get, list, presign), `object_requests`
//! (ranged reads and typed response headers), `upload_stream` (file and
//! stream bodies), `upload_verify` (upload, HEAD and digest check),
//! `upload_multipart` (managed multipart upload with cancellation),
//! `multipart_recovery` (manual multipart upload and error inspection),
//! `pagination` and `copy_prefix` (listing streams), `download` and
//! `download_memory` (the resumable download manager), `oauth` and
//! `oauth_inventory` (storage-token exchange). The repository README is the
//! complete usage manual.
//!
//! # Unsupported features
//!
//! These service capabilities are not provided: POST object creation,
//! archive restoration (`restoreArchive`; [`Client::restore_object`] restores
//! deleted objects only), multipart copy (`upload-part-copy` and the
//! `MULTI_BLOB` mode), list-parts based upload recovery, uploads of unknown
//! length, conditional writes and general versioned reads and writes. The
//! repository's `docs/protocol.md` explains the missing service evidence.

#[macro_use]
mod macros;

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
pub mod credentials;
#[cfg(feature = "manager")]
pub mod manager;
pub mod multipart;
pub mod oauth;
pub mod object;

pub use client::Client;
pub use config::{ClientBuilder, ClientConfiguration};
pub use error::{Dispatch, Error, ErrorKind, ResponseDetails, Result};
pub use retry::{RetryMode, RetryPolicy};
