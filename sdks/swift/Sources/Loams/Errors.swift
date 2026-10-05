// The typed error hierarchy of design §44 §7.4, decision D611.
//
// A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in its
// details. The `reason` is what callers branch on: a stable `snake_case` string,
// registered in `docs/api/reasons.md` and carried in ``Reason``, so a caller
// switches on a case and a reason the registry has lost stops compiling. The
// message is for people and may change; nothing in an SDK branches on it.
//
// In Swift the branch is `catch let error as LoamsError` and then a `switch` on
// `error.reason`, which is why the hierarchy exists:
//
//     do {
//         let info = try await client.instance.getInstance()
//     } catch let error as FeatureNotInVariantError {
//         // this build variant does not carry the package
//     } catch let error as LoamsError where error.reason == .notFound {
//         // the named resource does not exist
//     }
//
// and for a class rather than a reason:
//
//     } catch let error as UnauthenticatedError {
//         // the caller is not authenticated, for any reason
//     }
//
// Three cases are distinct and are not conflated:
//
//   - a reason from a **newer** server, which this SDK's registry does not have:
//     it is surfaced as text in ``LoamsError/unknownReason`` and flagged, not
//     dropped;
//   - a failure from **below the API** — a socket, a refused connection, a
//     cancelled task — which carries no `reason` at all;
//   - a ``LoamsError``, which is returned unchanged if it is mapped twice, so
//     wrapping an SDK's own error never loses its reason.
//
// # Swift's one structural difference from the other twelve SDKs
//
// Go and TypeScript both give every subclass a *distinct type* and let the
// language's error-matching machinery do the work. Swift's `catch` pattern
// matching over an `enum` does the same job with less ceremony, so this is one
// `enum` rather than a class per code. The consequence a caller should know: a
// `catch let error as FeatureNotInVariantError` only compiles because of the
// `errorLoamsPattern` helpers below, which destructure the enum — see
// ``FeatureNotInVariantError`` for why that indirection exists.

import Foundation

/// The Connect code: the canonical classification of a failure.
///
/// It does not change within a major version. `loams.dev/go` aliases
/// `connect.Code` so a caller can compare with either; Swift has no connect-go
/// to alias, so this is the SDK's own copy of the sixteen codes, which is what
/// makes the SDK dependency-free.
public enum Code: String, Sendable, Hashable, CaseIterable {
    case canceled
    case unknown
    case invalidArgument
    case deadlineExceeded = "deadline_exceeded"
    case notFound = "not_found"
    case alreadyExists = "already_exists"
    case permissionDenied = "permission_denied"
    case resourceExhausted = "resource_exhausted"
    case failedPrecondition = "failed_precondition"
    case aborted
    case outOfRange = "out_of_range"
    case unimplemented
    case `internal`
    case unavailable
    case dataLoss = "data_loss"
    case unauthenticated

    /// The wire name Connect uses for this code in a gRPC-Web trailer or a
    /// Connect HTTP status.
    public var wireName: String { rawValue }
}

/// The structured detail of a Loams failure, as `loams.errors.v1.ErrorInfo`.
///
/// A shape rather than the generated type, so a caller who wants the detail does
/// not have to import the stub package — the same reasoning as the Go SDK's
/// `ErrorInfoShape`.
public struct ErrorInfoShape: Sendable, Equatable {
    /// The stable, machine-readable cause.
    public let reason: String
    /// Structured context, for example `["variant": "standard"]`. Never carries
    /// secrets.
    public let metadata: [String: String]
    /// A short next step in the caller's locale.
    public let hint: String

    public init(reason: String, metadata: [String: String] = [:], hint: String = "") {
        self.reason = reason
        self.metadata = metadata
        self.hint = hint
    }
}

/// Every Loams failure, as one type.
///
/// The subclasses D611 names are distinguished by ``reason`` and by the
/// convenience accessors below rather than by being separate Swift types, with
/// one exception: ``FeatureNotInVariantError`` and ``TokenExpiredError`` get real
/// types, because R5 and R1 both make "branch on the type" the documented
/// affordance and a caller should not have to write `error.reason == .x` to get
/// it.
public enum LoamsError: Error, Sendable, Equatable {
    /// A failure that came from Loams and carries a code.
    ///
    /// `reason` is empty when the failure came from **below the API** — a socket,
    /// a timeout, a cancelled task — rather than from a Loams service. That is a
    /// different thing from a service refusing, and it is why `reason` is
    /// optional.
    case loams(
        code: Code,
        reason: Reason?,
        unknownReason: String?,
        metadata: [String: String],
        hint: String,
        rpc: String,
        message: String
    )

    /// A failure from below the API, which carries no code from the server.
    case transport(code: Code, rpc: String, message: String)

    /// A cancellation, which R2's retry policy must never answer.
    case cancelled(rpc: String)

    /// A deadline, which keeps its own code rather than hiding behind `unknown`.
    case deadlineExceeded(rpc: String)
}

/// A package this build variant does not carry (design §44 §4, D600).
///
/// The server answers `unimplemented` with `reason = feature_not_in_variant` and
/// names the variant in `metadata.variant`, which is what ``variant`` reads. A
/// caller usually never gets here: ``LoamsSystem/guard(_:)`` feature-detects from
/// `GetInstance.services[]` before calling, so an unavailable module raises *this
/// same type* from the guard with no request spent.
///
/// One `catch` therefore covers "the guard said no" and "the server refused",
/// which is the point of the guard raising the same type.
public struct FeatureNotInVariantError: Error, Sendable, Equatable {
    /// The underlying failure, with its code, reason, metadata and hint intact.
    public let base: LoamsError
    /// The build variant that was asked for, from `metadata.variant`.
    ///
    /// Empty if the server did not send one, and ``LoamsSystem/variantUnknown``
    /// when the SDK learned the package was absent from the catalogue rather than
    /// from a refusal.
    public let variant: String

    public init(base: LoamsError, variant: String) {
        self.base = base
        self.variant = variant
    }

    /// The stable reason, always `.featureNotInVariant`.
    public var reason: Reason { .featureNotInVariant }
}

/// A token the server rejected as expired: `unauthenticated` with reason
/// `token_expired`.
///
/// The runtime refreshes once and retries once (D608, R1); a second expiry
/// reaches the caller as this type.
public struct TokenExpiredError: Error, Sendable, Equatable {
    /// The underlying failure.
    public let base: LoamsError

    public init(base: LoamsError) {
        self.base = base
    }

    /// The stable reason, always `.tokenExpired`.
    public var reason: Reason { .tokenExpired }
}

// MARK: - Accessors

extension LoamsError {
    /// The code the failure carries, or `.unknown`.
    public var code: Code {
        switch self {
        case .loams(let code, _, _, _, _, _, _): return code
        case .transport(let code, _, _): return code
        case .cancelled: return .canceled
        case .deadlineExceeded: return .deadlineExceeded
        }
    }

    /// The reason the failure carries, or `nil`.
    ///
    /// `nil` for a failure from below the API, which is a different thing from a
    /// Loams service refusing.
    public var reason: Reason? {
        guard case .loams(_, let reason, _, _, _, _, _) = self else { return nil }
        return reason
    }

    /// A reason off the wire that this SDK's registry does not have, meaning the
    /// server is newer than the SDK.
    ///
    /// Surfaced rather than dropped: losing it would leave a caller unable to
    /// tell "not supported here" from "not supported at all" (R8).
    public var unknownReason: String? {
        guard case .loams(_, _, let unknown, _, _, _, _) = self else { return nil }
        return unknown
    }

    /// The structured context the server sent. Never secrets.
    public var metadata: [String: String] {
        guard case .loams(_, _, _, let metadata, _, _, _) = self else { return [:] }
        return metadata
    }

    /// A short next step in the caller's locale, when the server sent one.
    public var hint: String {
        guard case .loams(_, _, _, _, let hint, _, _) = self else { return "" }
        return hint
    }

    /// The RPC that failed, as `package.Service/Method`.
    public var rpc: String {
        switch self {
        case .loams(_, _, _, _, _, let rpc, _): return rpc
        case .transport(_, let rpc, _): return rpc
        case .cancelled(let rpc): return rpc
        case .deadlineExceeded(let rpc): return rpc
        }
    }

    /// The human-readable message, for a log or a support ticket. Never branch
    /// on it.
    public var message: String {
        switch self {
        case .loams(_, _, _, _, _, _, let message): return message
        case .transport(_, _, let message): return message
        case .cancelled: return "the call was cancelled"
        case .deadlineExceeded: return "the call's deadline expired"
        }
    }

    /// Whether the failure came from Loams rather than from below the API.
    public var isLoamsFailure: Bool {
        if case .loams = self { return true }
        return false
    }

    /// The reason an error carries, or `nil` when it carries none.
    ///
    /// The free function, because a caller holding an `any Error` cannot reach
    /// the enum case without a `catch` first, and `catch` is where the SDK
    /// usually is not.
    public static func reason(of error: any Error) -> Reason? {
        if let loams = error as? LoamsError { return loams.reason }
        if let absent = error as? FeatureNotInVariantError { return absent.reason }
        if let expired = error as? TokenExpiredError { return expired.reason }
        return nil
    }

    /// The code an error carries, or `.unknown`.
    public static func code(of error: any Error) -> Code {
        if let loams = error as? LoamsError { return loams.code }
        if let absent = error as? FeatureNotInVariantError { return absent.base.code }
        if let expired = error as? TokenExpiredError { return expired.base.code }
        return .unknown
    }

    /// The RPC an error names, or `""`.
    public static func rpc(of error: any Error) -> String {
        if let loams = error as? LoamsError { return loams.rpc }
        if let absent = error as? FeatureNotInVariantError { return absent.base.rpc }
        if let expired = error as? TokenExpiredError { return expired.base.rpc }
        return ""
    }

    /// Whether an error came from Loams rather than from below the API.
    public static func isLoamsError(_ error: any Error) -> Bool {
        error is LoamsError || error is FeatureNotInVariantError || error is TokenExpiredError
    }
}

// MARK: - Construction

extension LoamsError {
    /// The SDK's own failures — a binding that does not resolve, a request whose
    /// shape it cannot read.
    ///
    /// `internal` because they are bugs in this package, not in the caller or the
    /// server, which is exactly what D611 says `internal` means.
    static func internalError(_ rpc: String, _ message: String) -> LoamsError {
        .loams(
            code: .internal,
            reason: .internalReason,
            unknownReason: nil,
            metadata: [:],
            hint: "",
            rpc: rpc,
            message: message
        )
    }
}

// MARK: - Mapping

extension LoamsError {
    /// Turns any error into the typed hierarchy.
    ///
    /// A wire failure becomes the class its code names, with `reason` and
    /// `metadata` lifted out of the `ErrorInfo` detail. Anything from **below the
    /// API** — a socket, a refused connection, a cancelled task, a bug in the
    /// SDK — becomes `.transport(code: .unknown)` or `.cancelled`, **except** that
    /// a deadline keeps its own code, because a deadline that expires is a
    /// deadline and hiding it behind `unknown` would lose the one thing the caller
    /// can always act on.
    ///
    /// An error that is **already** a ``LoamsError`` (or one of its two typed
    /// subclasses) is returned unchanged, so mapping twice loses nothing.
    static func map(_ error: any Error, rpc: String) -> any Error {
        // Already ours: return it untouched. This is R8's "returned unchanged if
        // mapped twice", and it is what stops a wrapping call from dropping the
        // reason.
        if LoamsError.isLoamsError(error) { return error }

        // A cancellation is not a failure and must not be dressed as one: R2's
        // retry policy reads `.canceled` and refuses to retry it, so flattening it
        // into `.unknown` would make the policy retry a call the caller gave up.
        if error is CancellationError { return LoamsError.cancelled(rpc: rpc) }

        let nsError = error as NSError
        if nsError.domain == NSURLErrorDomain || nsError.domain == NSPOSIXErrorDomain {
            if nsError.code == NSURLErrorCancelled {
                return LoamsError.cancelled(rpc: rpc)
            }
            if nsError.code == NSURLErrorTimedOut {
                return LoamsError.deadlineExceeded(rpc: rpc)
            }
            return LoamsError.transport(code: .unknown, rpc: rpc, message: nsError.localizedDescription)
        }
        if #available(macOS 13.0, iOS 16.0, *) {
            if error is ConcurrencyError || error is ClockError {
                return LoamsError.transport(code: .unknown, rpc: rpc, message: "\(error)")
            }
        }
        return LoamsError.transport(code: .unknown, rpc: rpc, message: "\(error)")
    }

    /// Builds the failure a wire response produced.
    ///
    /// `info` is the `ErrorInfo` detail, looked up **by type**; it is `nil` when
    /// the server sent no detail, when the detail's bytes did not parse, or when
    /// the detail was some other type. All three are reported as "no reason"
    /// rather than dropped silently, because the alternative is a Loams failure
    /// with no reason and no hint — the one thing R8 says must not happen.
    static func fromWire(
        code: Code,
        info: ErrorInfoShape?,
        rpc: String,
        message: String
    ) -> any Error {
        var reason: Reason?
        var unknownReason: String?
        var metadata: [String: String] = [:]
        var hint = ""

        if let info {
            if let known = loamsReason(from: info.reason) {
                reason = known
            } else if !info.reason.isEmpty {
                unknownReason = info.reason
            }
            metadata = info.metadata
            if !info.hint.isEmpty { hint = info.hint }
        }

        let base = LoamsError.loams(
            code: code,
            reason: reason,
            unknownReason: unknownReason,
            metadata: metadata,
            hint: hint,
            rpc: rpc,
            message: message
        )

        // R5's dedicated type, checked **before** the code class, because
        // `feature_not_in_variant` arrives as `unimplemented` and a caller
        // branching on the class would otherwise have to also check the reason.
        if reason == .featureNotInVariant {
            return FeatureNotInVariantError(base: base, variant: metadata["variant"] ?? "")
        }
        // R1's second expiry.
        if code == .unauthenticated && reason == .tokenExpired {
            return TokenExpiredError(base: base)
        }
        return base
    }
}

// MARK: - Description

extension LoamsError: CustomStringConvertible {
    public var description: String {
        let prefix = rpc.isEmpty ? "" : rpc + ": "
        let reasonText = reason?.rawValue ?? unknownReason ?? ""
        guard !reasonText.isEmpty else {
            return prefix + code.rawValue + ": " + message
        }
        return prefix + code.rawValue + " (" + reasonText + "): " + message
    }
}

extension FeatureNotInVariantError: CustomStringConvertible {
    public var description: String {
        "loams: " + variantLabel + " does not carry the package this call needs"
    }

    /// The variant, or the honest `unknown`.
    ///
    /// The SDK learned this from the catalogue rather than from a refusal, so it
    /// does not know the variant; saying so beats printing a plausible-looking
    /// one in a support ticket.
    private var variantLabel: String { variant.isEmpty ? LoamsSystem.variantUnknown : variant }
}

extension TokenExpiredError: CustomStringConvertible {
    public var description: String {
        "\(base): the token was refreshed once and the server rejected the new one too"
    }
}