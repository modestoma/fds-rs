//! High-level helpers built on top of [`crate::Client`].
//! Enabled by the default `manager` feature.

mod download;

pub use download::{DownloadOptions, DownloadOptionsBuilder, DownloadRequest, Downloader};
