//! Query-string encoding. Ports `fds/httpparser/`. Crate-internal: the
//! byte-exact encoding is an implementation detail.

mod query;

pub(crate) use query::{go_query_escape, QueryValues};
