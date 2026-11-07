//! Struct → query-string / header / range encoders. Ports `fds/httpparser/`.

pub mod header;
pub mod query;
pub mod range;

pub use header::ToHeaders;
pub use query::{format_rfc822, go_query_escape, QueryValues, ToQuery};
pub use range::{parse_range, HttpRange};
