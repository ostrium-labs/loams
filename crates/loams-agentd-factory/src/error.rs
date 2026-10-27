//! What a factory op can fail with, and how a failure maps to the wire's `code`
//! (the TypeScript host's `classifyError`, ported).

/// What kind of failure an [`OpError`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpErrorKind {
    /// The upstream answered with an HTTP status, or a body that could not be read.
    Http,
    /// The request never got an answer (refused, reset, DNS, TLS).
    Network,
    /// The request took too long.
    Timeout,
    /// Matomo's error inside a 200 body (`{"result":"error"}`).
    MatomoApi,
    /// A missing non-secret field (`projectKey`, `idSite`): `bad_params`.
    Param,
    /// An op this adapter does not serve: `unsupported`.
    Unsupported,
    /// Anything else the adapter refused (a body of the wrong shape, an app's own
    /// error envelope without a status).
    Other,
}

/// A failed op. `status` is the upstream's HTTP status when there was one (0 for a
/// network error, as the TypeScript client reports it).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct OpError {
    pub kind: OpErrorKind,
    pub status: Option<u16>,
    pub message: String,
}

impl OpError {
    pub fn http(status: u16, message: impl Into<String>) -> Self {
        Self {
            kind: OpErrorKind::Http,
            status: Some(status),
            message: message.into(),
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self {
            kind: OpErrorKind::Network,
            status: Some(0),
            message: message.into(),
        }
    }

    pub fn timeout() -> Self {
        Self {
            kind: OpErrorKind::Timeout,
            status: None,
            message: "request timed out".into(),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self {
            kind: OpErrorKind::Other,
            status: None,
            message: message.into(),
        }
    }

    pub fn param(message: impl Into<String>) -> Self {
        Self {
            kind: OpErrorKind::Param,
            status: None,
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            kind: OpErrorKind::Unsupported,
            status: None,
            message: message.into(),
        }
    }

    /// Matomo's error inside a 200 body.
    pub fn matomo(message: impl Into<String>) -> Self {
        Self {
            kind: OpErrorKind::MatomoApi,
            status: Some(200),
            message: message.into(),
        }
    }

    /// An HTTP 404: what the ops treat as "this version does not serve that route".
    pub fn is_not_found(&self) -> bool {
        self.status == Some(404)
    }

    /// The wire's `code` for a query that failed with this error.
    pub fn code(&self) -> &'static str {
        match self.kind {
            OpErrorKind::Param => "bad_params",
            OpErrorKind::Unsupported => "unsupported",
            _ => classify(self),
        }
    }
}

/// `auth_failed`, `unreachable` or `upstream_error`, as `host.ts`'s `classifyError`.
fn classify(e: &OpError) -> &'static str {
    match e.status {
        Some(401 | 403) => return "auth_failed",
        Some(0) => return "unreachable",
        _ => {}
    }
    let matomo_200 = e.kind == OpErrorKind::MatomoApi && e.status == Some(200);
    if e.status.is_some() && !matomo_200 {
        return "upstream_error";
    }
    let lower = e.message.to_ascii_lowercase();
    if e.kind == OpErrorKind::MatomoApi
        && ["token", "auth", "permission", "access", "credential"]
            .iter()
            .any(|w| lower.contains(w))
    {
        return "auth_failed";
    }
    if ["invalid api key", "unauthorized", "forbidden"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return "auth_failed";
    }
    if matches!(e.kind, OpErrorKind::Network | OpErrorKind::Timeout) {
        return "unreachable";
    }
    "upstream_error"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_follow_the_typescript_classifier() {
        assert_eq!(OpError::http(401, "x").code(), "auth_failed");
        assert_eq!(OpError::http(403, "x").code(), "auth_failed");
        assert_eq!(OpError::http(500, "x").code(), "upstream_error");
        assert_eq!(OpError::http(404, "x").code(), "upstream_error");
        assert_eq!(OpError::network("refused").code(), "unreachable");
        assert_eq!(OpError::timeout().code(), "unreachable");
        assert_eq!(
            OpError::matomo("You must be logged in").code(),
            "upstream_error"
        );
        assert_eq!(
            OpError::matomo("token_auth is not valid").code(),
            "auth_failed"
        );
        assert_eq!(OpError::other("Invalid API key").code(), "auth_failed");
        assert_eq!(OpError::other("not an envelope").code(), "upstream_error");
        assert_eq!(
            OpError::param("projectKey is required").code(),
            "bad_params"
        );
        assert_eq!(OpError::unsupported("no").code(), "unsupported");
    }
}
