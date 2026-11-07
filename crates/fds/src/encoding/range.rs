//! HTTP Range parsing. Ports `fds/httpparser/range.go`.

use crate::error::{Error, Result};

/// A single byte range (inclusive `start`..=`end`), mirroring Go `HTTPRange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpRange {
    pub start: i64,
    pub end: i64,
}

/// Parse an HTTP Range header value into ranges (Go `httpparser.Range`).
///
/// Rules (range.go):
/// - empty string → empty vec, ok;
/// - must contain `=`; the left side must be the literal `bytes`;
/// - the right side is split by `,` into items; each item split by `-` must be
///   exactly 2 parts;
/// - `start`: empty → 0, else decimal;
/// - `end`: **must not be empty** (empty → error, so `bytes=100-` is
///   unsupported); else decimal.
pub fn parse_range(r: &str) -> Result<Vec<HttpRange>> {
    if r.is_empty() {
        return Ok(Vec::new());
    }

    let index = match r.find('=') {
        Some(i) => i,
        None => return Err(Error::invalid_input("fds: error range format")),
    };

    if &r[..index] != "bytes" {
        return Err(Error::invalid_input("fds: only support bytes range"));
    }

    let mut ranges = Vec::new();
    for item in r[index + 1..].split(',') {
        let parts: Vec<&str> = item.split('-').collect();
        if parts.len() != 2 {
            return Err(Error::invalid_input("fds: error range format"));
        }

        let start: i64 = if parts[0].is_empty() {
            0
        } else {
            parts[0]
                .parse()
                .map_err(|_| Error::invalid_input("fds: error range format"))?
        };

        if parts[1].is_empty() {
            return Err(Error::invalid_input("fds: error range format"));
        }
        let end: i64 = parts[1]
            .parse()
            .map_err(|_| Error::invalid_input("fds: error range format"))?;

        ranges.push(HttpRange { start, end });
    }

    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        assert_eq!(parse_range("").unwrap(), vec![]);
    }

    #[test]
    fn single() {
        assert_eq!(
            parse_range("bytes=0-99").unwrap(),
            vec![HttpRange { start: 0, end: 99 }]
        );
    }

    #[test]
    fn multi_and_empty_start() {
        // Golden vector from range_test.go: TestRange("bytes=-1,2-4,-5")
        assert_eq!(
            parse_range("bytes=-1,2-4,-5").unwrap(),
            vec![
                HttpRange { start: 0, end: 1 },
                HttpRange { start: 2, end: 4 },
                HttpRange { start: 0, end: 5 },
            ]
        );
    }

    #[test]
    fn open_ended_end_errors() {
        // `bytes=100-` (to end) is NOT supported by Go.
        assert!(parse_range("bytes=100-").is_err());
    }

    #[test]
    fn wrong_unit_errors() {
        assert!(parse_range("items=0-1").is_err());
    }

    #[test]
    fn no_eq_errors() {
        assert!(parse_range("bytes0-1").is_err());
    }
}
