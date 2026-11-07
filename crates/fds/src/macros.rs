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

/// The `retry` option of an idempotent write's builder; the builder needs a
/// `retry: crate::RetryMode` field and passes it to
/// `ClientRequest::idempotent`.
macro_rules! retry_option {
    () => {
        /// Override the client's [`RetryPolicy`](crate::RetryPolicy) for this
        /// idempotent write: [`RetryMode::Enabled`](crate::RetryMode::Enabled)
        /// retries it even when the policy does not
        /// [`retry_idempotent_writes`](crate::RetryPolicy::retry_idempotent_writes),
        /// [`RetryMode::Disabled`](crate::RetryMode::Disabled) sends it once.
        ///
        /// The number of retries and their delays still come from the
        /// policy, so a client without retries never retries. A stream or file
        /// body is never retried.
        pub fn retry(mut self, mode: crate::RetryMode) -> Self {
            self.retry = mode;
            self
        }
    };
}
