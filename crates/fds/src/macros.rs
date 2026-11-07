//! Crate-internal macros shared by the operation builders of several domain
//! modules. Declared first in `lib.rs` so every module can use them.

/// The `timeout` option shared by every operation builder; the builder needs a
/// `timeout: Option<Duration>` field.
macro_rules! timeout_option {
    () => {
        /// Set a total deadline for this request only, overriding the
        /// client-wide [`request_timeout`](crate::ClientBuilder::request_timeout)
        /// whether that is set or not.
        ///
        /// The deadline runs from connecting until the response has been read.
        /// When the operation is retried it applies to each attempt
        /// separately. The client's connect timeout and idle
        /// [`read_timeout`](crate::ClientBuilder::read_timeout) still apply.
        /// Must be nonzero and representable; otherwise `send` fails with
        /// [`ErrorKind::InvalidInput`](crate::ErrorKind::InvalidInput).
        pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
            self.timeout = Some(timeout);
            self
        }
    };
}
