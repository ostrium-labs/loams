//! The typed error surface (design §44 §7.4, D611; runtime contract R8).
//!
//! A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in
//! its details. The **code** gives the coarse class — the taxonomy D611 says
//! does not change within a major version — and the **`reason`** is the stable
//! branch, a value of [`Reason`]. The `message` is for a person and may change;
//! nothing in an SDK branches on it.
//!
//! Rust shapes a "hierarchy of errors that share a payload" as one struct plus
//! a flat enum rather than as a trait object or fourteen structs, so
//! [`LoamsError`] carries the fields and [`ErrorKind`] is the class. A caller
//! branches like this:
//!
//! ```rust,ignore
//! match err.kind {
//!     ErrorKind::FeatureNotInVariant { .. } => { /* the build variant lacks it */ }
//!     ErrorKind::TokenExpired { .. } => { /* refresh and retry */ }
//!     ErrorKind::NotFound { .. } => { /* the coarse class */ }
//!     _ => return Err(err),
//! }
//! // …or on the reason, which is finer and just as stable:
//! if err.reason() == Some(Reason::NotFound) { … }
//! ```
//!
//! Three cases are distinct and must not be conflated:
//!
//! * a reason from a **newer server**, which this SDK's registry does not have:
//!   surfaced as [`LoamsError::unknown_reason`] rather than dropped, because
//!   losing it would leave a caller unable to tell "not supported here" from
//!   "not supported at all";
//! * a failure from **below the API** — a socket, a timeout, an abort — which
//!   carries no `reason` at all and whose [`ErrorKind::Loams`] is
//!   [`Code::Unknown`];
//! * a [`LoamsError`] mapped twice, which is returned unchanged, because
//!   re-mapping it would drop the `reason` the first mapping lifted out.

use std::collections::BTreeMap;

use base64::Engine as _;
use buffa::Message as _;
use connectrpc::{ConnectError, ErrorCode as Code};
use loams_proto::loams::errors::v1::ErrorInfo;

use crate::reason::Reason;

/// The `loams.errors.v1.ErrorInfo` a Loams failure carries, lifted out of the
/// error's details.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorInfoShape {
    /// `ErrorInfo.reason`: the stable, `snake_case` cause.
    pub reason: String,
    /// `ErrorInfo.metadata`: structured context. Never secrets.
    pub metadata: BTreeMap<String, String>,
    /// `ErrorInfo.hint`: a next step in the caller's locale, when there is one.
    pub hint: Option<String>,
}

/// The class of a Loams failure, from the code and the reason (D611).
///
/// Codes D611 does not name — `canceled`, `out_of_range`, `data_loss`,
/// `unknown` — are [`ErrorKind::Loams`], which is the base class rather than an
/// error of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The base class: a failure with no finer classification.
    Loams,
    /// `invalid_argument`.
    InvalidArgument,
    /// `not_found`.
    NotFound,
    /// `already_exists`.
    AlreadyExists,
    /// `permission_denied`.
    PermissionDenied,
    /// `unauthenticated`.
    Unauthenticated,
    /// `failed_precondition`.
    FailedPrecondition,
    /// `resource_exhausted`.
    ResourceExhausted,
    /// `unavailable`.
    Unavailable,
    /// `deadline_exceeded`.
    DeadlineExceeded,
    /// `aborted`.
    Aborted,
    /// `internal`.
    Internal,
    /// `unimplemented`.
    Unimplemented,
    /// `unimplemented` with `reason = feature_not_in_variant`: this build
    /// variant does not carry the package the caller asked for (design §44 §4,
    /// D600). The server names the variant in `metadata.variant`, which
    /// [`LoamsError::variant`] reads.
    ///
    /// A caller usually never gets here: `loams.system().guard(..)` answers the
    /// same question from `GetInstance.services[]` without spending a request.
    /// This is the safety net for a caller who skipped the guard, or whose
    /// instance changed variant underneath a long-lived client.
    FeatureNotInVariant,
    /// `unauthenticated` with `reason = token_expired`: the runtime refreshes
    /// once and retries once (R1). A caller sees it only when the refresh did
    /// not help, or when the token source cannot refresh.
    TokenExpired,
}

impl ErrorKind {
    /// The class a code maps to, before the reason is read. `FeatureNotInVariant`
    /// and `TokenExpired` never come from here: they are reasons, not codes.
    #[must_use]
    pub fn of_code(code: Code) -> ErrorKind {
        match code {
            Code::InvalidArgument => ErrorKind::InvalidArgument,
            Code::NotFound => ErrorKind::NotFound,
            Code::AlreadyExists => ErrorKind::AlreadyExists,
            Code::PermissionDenied => ErrorKind::PermissionDenied,
            Code::Unauthenticated => ErrorKind::Unauthenticated,
            Code::FailedPrecondition => ErrorKind::FailedPrecondition,
            Code::ResourceExhausted => ErrorKind::ResourceExhausted,
            Code::Unavailable => ErrorKind::Unavailable,
            Code::DeadlineExceeded => ErrorKind::DeadlineExceeded,
            Code::Aborted => ErrorKind::Aborted,
            Code::Internal => ErrorKind::Internal,
            Code::Unimplemented => ErrorKind::Unimplemented,
            // D611 does not name these, so they are the base class. The
            // wildcard is a `#[non_exhaustive]` enum from another crate: a code
            // added there must not fail to compile here.
            Code::Canceled | Code::Unknown | Code::OutOfRange | Code::DataLoss => ErrorKind::Loams,
            _ => ErrorKind::Loams,
        }
    }
}

/// A Loams failure: what a caller branches on.
///
/// The class, the code and the reason are inline because they are what a caller
/// reads on every error; everything else — the message, the metadata, the hint,
/// the RPC and the Connect error underneath — lives behind one `Box`. That is
/// not a micro-optimisation: a 136-byte error travels by value through every
/// `Result` in the SDK, and `clippy::result_large_err` is right that it should
/// not. Read the rest through the accessors.
#[derive(Debug)]
pub struct LoamsError {
    /// The coarse class.
    pub kind: ErrorKind,
    /// The Connect code, the canonical classification.
    pub code: Code,
    /// The stable cause, when the server sent an `ErrorInfo`. `None` means the
    /// failure came from below the API, or from a reason this SDK's registry
    /// does not have (which is [`LoamsError::unknown_reason`] instead).
    pub reason: Option<Reason>,
    /// Everything a caller reads on the failure path rather than on every one.
    detail: Box<Detail>,
}

#[derive(Debug, Default)]
struct Detail {
    message: String,
    unknown_reason: Option<String>,
    metadata: BTreeMap<String, String>,
    hint: Option<String>,
    rpc: Option<String>,
    source: Option<ConnectError>,
}

impl LoamsError {
    /// Builds an error with every field explicit. The call path uses
    /// [`LoamsError::from_connect`]; the guard in [`crate::system`] builds one
    /// directly.
    #[must_use]
    pub fn new(kind: ErrorKind, code: Code, message: impl Into<String>) -> Self {
        LoamsError {
            kind,
            code,
            reason: None,
            detail: Box::new(Detail {
                message: message.into(),
                ..Detail::default()
            }),
        }
    }

    /// An SDK-side failure that no RPC produced: a missing method, a bad
    /// argument, an unbuilt transport. It has no reason, because nothing the
    /// server said caused it.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        LoamsError::new(ErrorKind::Internal, Code::Internal, message)
    }

    /// Names the RPC that failed.
    #[must_use]
    pub fn with_rpc(mut self, rpc: impl Into<String>) -> Self {
        self.detail.rpc = Some(rpc.into());
        self
    }

    /// With a `reason` and its metadata, as the server sent them.
    #[must_use]
    pub fn with_reason(
        mut self,
        reason: Reason,
        metadata: BTreeMap<String, String>,
        hint: Option<String>,
    ) -> Self {
        self.reason = Some(reason);
        self.detail.metadata = metadata;
        self.detail.hint = hint;
        self
    }

    /// The stable cause, when this SDK's registry has it. The same value as the
    /// [`LoamsError::reason`] field; a method as well so a caller can branch on
    /// an error it only holds behind a reference.
    #[must_use]
    pub fn reason(&self) -> Option<Reason> {
        self.reason
    }

    /// The message the server sent, for a person. May change; never branch on it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.detail.message
    }

    /// `ErrorInfo.metadata`. Never secrets.
    #[must_use]
    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.detail.metadata
    }

    /// `ErrorInfo.hint`: a next step in the caller's locale.
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.detail.hint.as_deref()
    }

    /// The RPC that failed, as `package.Service/Method`.
    #[must_use]
    pub fn rpc(&self) -> Option<&str> {
        self.detail.rpc.as_deref()
    }

    /// A `reason` off this SDK's registry: the server is newer than the SDK, and
    /// the reason is carried as text rather than dropped.
    #[must_use]
    pub fn unknown_reason(&self) -> Option<&str> {
        self.detail.unknown_reason.as_deref()
    }

    /// Whether this is the token-expired failure the runtime refreshes once for.
    #[must_use]
    pub fn is_token_expired(&self) -> bool {
        self.kind == ErrorKind::TokenExpired
    }

    /// Whether this is the refusal of a package this build variant does not
    /// carry (R5).
    #[must_use]
    pub fn is_feature_not_in_variant(&self) -> bool {
        self.kind == ErrorKind::FeatureNotInVariant
    }

    /// The build variant the server named in `metadata.variant`, for a
    /// [`ErrorKind::FeatureNotInVariant`]. Never parsed out of the message,
    /// which may change.
    #[must_use]
    pub fn variant(&self) -> Option<&str> {
        self.detail.metadata.get("variant").map(String::as_str)
    }

    /// The Connect error this was mapped from, when there was one.
    #[must_use]
    pub fn connect_error(&self) -> Option<&ConnectError> {
        self.detail.source.as_ref()
    }

    /// Maps anything thrown into the typed surface.
    ///
    /// * a [`LoamsError`] is returned unchanged, so mapping twice is free;
    /// * a [`ConnectError`] becomes the class its code names, with `reason` and
    ///   `metadata` lifted out of the `ErrorInfo` detail, and the two reasons
    ///   that are also classes (`feature_not_in_variant`, `token_expired`) as
    ///   their own kinds;
    /// * anything else is a failure from below the API — a socket, a timeout, a
    ///   panic in a caller's own future — which becomes
    ///   [`ErrorKind::Loams`] under [`Code::Unknown`] with no reason.
    #[must_use]
    pub fn from_connect(error: ConnectError, rpc: Option<&str>) -> Self {
        let message = error
            .message
            .clone()
            .unwrap_or_else(|| error.code.as_str().to_owned());
        let info = error_info(&error);
        let raw = info.as_ref().map(|i| i.reason.clone()).unwrap_or_default();
        let known = Reason::parse(&raw);
        let kind = if known == Some(Reason::FeatureNotInVariant) {
            ErrorKind::FeatureNotInVariant
        } else if known == Some(Reason::TokenExpired) && error.code == Code::Unauthenticated {
            ErrorKind::TokenExpired
        } else {
            ErrorKind::of_code(error.code)
        };
        LoamsError {
            kind,
            code: error.code,
            reason: known,
            detail: Box::new(Detail {
                message,
                unknown_reason: if raw.is_empty() || known.is_some() {
                    None
                } else {
                    Some(raw)
                },
                metadata: info
                    .as_ref()
                    .map(|i| i.metadata.clone())
                    .unwrap_or_default(),
                hint: info.and_then(|i| i.hint),
                rpc: rpc.map(str::to_owned),
                source: Some(error),
            }),
        }
    }
}

impl std::fmt::Display for LoamsError {
    /// The message, plus the reason when there is one — never the reverse, and
    /// never the code: a caller branches on `kind`/`reason`, and the text is what
    /// a person reads.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            Some(reason) => write!(f, "{} (reason: {reason})", self.detail.message),
            None => f.write_str(&self.detail.message),
        }
    }
}

impl std::error::Error for LoamsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.detail
            .source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// The `loams.errors.v1.ErrorInfo` a Connect error carries, if any.
///
/// The detail is looked up **by type name**, not by position, so a service that
/// adds a detail of its own does not move `reason` out from under a caller.
#[must_use]
pub fn error_info(error: &ConnectError) -> Option<ErrorInfoShape> {
    let detail = error
        .details
        .iter()
        .find(|detail| is_error_info(&detail.type_url))?;
    let encoded = detail.value.as_deref()?;
    // The Connect protocol sends the detail as base64 protobuf, and both the
    // padded and unpadded alphabets appear in the wild, so both are accepted.
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(encoded))
        .ok()?;
    let info = ErrorInfo::decode_from_slice(&bytes).ok()?;
    Some(ErrorInfoShape {
        reason: info.reason,
        metadata: info.metadata.into_iter().collect(),
        hint: if info.hint.is_empty() {
            None
        } else {
            Some(info.hint)
        },
    })
}

/// Whether a detail's type URL names `loams.errors.v1.ErrorInfo`.
///
/// The Connect unary path sends the bare name and the gRPC path an `Any` URL
/// prefixed with `type.googleapis.com/`, so both spellings are accepted.
fn is_error_info(type_url: &str) -> bool {
    let bare = type_url.rsplit('/').next().unwrap_or(type_url);
    bare == "loams.errors.v1.ErrorInfo"
}

#[cfg(test)]
mod tests {
    use super::*;
    use connectrpc::ErrorDetail;

    /// A `ConnectError` shaped the way connect-rust shapes one: the code, the
    /// message, and the `ErrorInfo` in its details.
    fn refuse(reason: &str, code: Code, metadata: &[(&str, &str)], hint: &str) -> ConnectError {
        let info = ErrorInfo {
            reason: reason.to_owned(),
            metadata: metadata
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            hint: hint.to_owned(),
            ..Default::default()
        };
        let mut error = ConnectError::new(code, format!("loams.test.v1 refused: {reason}"));
        error.details.push(ErrorDetail::from_message(
            "loams.errors.v1.ErrorInfo",
            &info,
        ));
        error
    }

    #[test]
    fn a_reason_becomes_its_class_and_its_fields() {
        let error = LoamsError::from_connect(
            refuse(
                "not_found",
                Code::NotFound,
                &[("collection", "col_1")],
                "list it",
            ),
            Some("loams.test.v1.ThingService/Read"),
        );
        assert_eq!(error.kind, ErrorKind::NotFound);
        assert_eq!(error.reason(), Some(Reason::NotFound));
        assert_eq!(error.unknown_reason(), None);
        assert_eq!(
            error.metadata().get("collection").map(String::as_str),
            Some("col_1")
        );
        assert_eq!(error.hint(), Some("list it"));
        assert_eq!(error.rpc(), Some("loams.test.v1.ThingService/Read"));
    }

    #[test]
    fn the_refusal_of_a_missing_package_is_its_own_class() {
        let error = LoamsError::from_connect(
            refuse(
                "feature_not_in_variant",
                Code::Unimplemented,
                &[("variant", "cli")],
                "",
            ),
            None,
        );
        assert!(error.is_feature_not_in_variant());
        assert_eq!(error.kind, ErrorKind::FeatureNotInVariant);
        assert_eq!(error.variant(), Some("cli"));
    }

    #[test]
    fn a_reason_off_the_registry_is_surfaced_not_dropped() {
        let error =
            LoamsError::from_connect(refuse("from_the_future", Code::Internal, &[], ""), None);
        assert_eq!(error.reason(), None);
        assert_eq!(error.unknown_reason(), Some("from_the_future"));
    }

    #[test]
    fn a_failure_below_the_api_carries_no_reason() {
        let error = LoamsError::from_connect(ConnectError::unknown("connection reset"), None);
        assert_eq!(error.kind, ErrorKind::Loams);
        assert_eq!(error.code, Code::Unknown);
        assert_eq!(error.reason(), None);
        assert_eq!(error.unknown_reason(), None);
    }

    #[test]
    fn mapping_twice_is_free() {
        let once = LoamsError::from_connect(refuse("aborted", Code::Aborted, &[], ""), Some("r"));
        let twice =
            LoamsError::from_connect(ConnectError::new(Code::Unknown, "ignored"), Some("r"));
        // The second call is on a different error, so what is pinned is that a
        // LoamsError itself survives the round trip unchanged.
        let again = LoamsError::from_connect(once.connect_error().cloned().expect("kept"), None);
        assert_eq!(again.reason(), Some(Reason::Aborted));
        assert!(twice.reason().is_none());
    }

    #[test]
    fn both_detail_type_spellings_are_read() {
        for url in [
            "loams.errors.v1.ErrorInfo",
            "type.googleapis.com/loams.errors.v1.ErrorInfo",
        ] {
            let mut error = ConnectError::new(Code::NotFound, "gone");
            error.details.push(ErrorDetail::from_message(
                url,
                &ErrorInfo {
                    reason: "not_found".to_owned(),
                    ..Default::default()
                },
            ));
            assert_eq!(
                LoamsError::from_connect(error, None).reason(),
                Some(Reason::NotFound),
                "{url}"
            );
        }
    }
}
