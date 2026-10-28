//! The errors of the Flink clients and the controller's calls into them.

/// What a call to the SQL Gateway or the JobManager REST API returned
/// instead of an answer.
#[derive(Debug, thiserror::Error)]
pub enum FlinkError {
    /// The request never got an HTTP answer (connection, TLS, timeout).
    #[error("request to {url} failed: {source}")]
    Transport {
        /// The request's URL.
        url: String,
        /// The client's error.
        #[source]
        source: reqwest::Error,
    },
    /// Flink answered with an error status. Flink's body is
    /// `{"errors": ["...", ...]}`; `message` joins them (shortened).
    #[error("HTTP {status} from {url}: {message}")]
    Api {
        /// The request's URL.
        url: String,
        /// The HTTP status.
        status: u16,
        /// Flink's `errors`, each shortened to [`MAX_ERROR_LEN`] bytes.
        errors: Vec<String>,
        /// `errors` joined, for display.
        message: String,
    },
    /// The answer was not the JSON this client expects.
    #[error("unexpected response from {url}: {message}")]
    Decode {
        /// The request's URL.
        url: String,
        /// What did not parse.
        message: String,
    },
    /// The gateway does not speak the API version this call needs.
    #[error("{0}")]
    Unsupported(String),
    /// An argument this client refuses before sending (a malformed handle,
    /// job id or URL).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// A wait (results, savepoint, checkpoint) ran past its deadline.
    #[error("timed out: {0}")]
    Timeout(String),
    /// Flink reports that the operation failed (a statement in `ERROR`, a
    /// savepoint with a failure cause).
    #[error("operation failed: {0}")]
    Failed(String),
}

/// The longest Flink error string kept (Flink sends whole stack traces).
pub const MAX_ERROR_LEN: usize = 2048;

impl FlinkError {
    /// The HTTP status of an [`FlinkError::Api`] error.
    pub fn status(&self) -> Option<u16> {
        match self {
            FlinkError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Whether Flink answered 404 (an unknown session, operation, job or
    /// trigger).
    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }
}

/// `s` cut to at most `max` bytes on a character boundary.
pub(crate) fn shorten(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_keeps_char_boundaries() {
        assert_eq!(shorten("abc", 10), "abc");
        assert_eq!(shorten("abcdef", 3), "abc…");
        // "é" is two bytes: cutting inside it backs off to the boundary.
        assert_eq!(shorten("aé", 2), "a…");
    }
}
