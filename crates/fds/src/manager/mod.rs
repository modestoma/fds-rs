//! Managed file downloads built on top of [`crate::Client`]: see
//! [`Client::download_file`](crate::Client::download_file).
//! Enabled by the default `manager` feature.

mod download;

pub use download::{DownloadFile, RequestLimiter};
