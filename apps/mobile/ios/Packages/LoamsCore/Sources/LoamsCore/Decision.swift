import Foundation

public enum Decision: String, Sendable {
    case approve
    case reject
}

/// What a decision proof signs (design §37 §7.3, AP0 Ruling 7): keys in byte order, no
/// whitespace, integers in decimal, strings escaped as RFC 8785. The bytes must equal Android's
/// and the mock's (conformance/fixtures/decision/claims.json).
public struct DecisionClaims: Sendable, Equatable {
    public let approvalID: String
    public let revision: UInt64
    public let decision: Decision
    public let iat: Int64
    public let jti: String

    public init(approvalID: String, revision: UInt64, decision: Decision, iat: Int64, jti: String) {
        self.approvalID = approvalID
        self.revision = revision
        self.decision = decision
        self.iat = iat
        self.jti = jti
    }

    public func canonicalJSON() -> Data {
        var s = "{\"approval_id\":" + Self.quote(approvalID)
        s += ",\"decision\":" + Self.quote(decision.rawValue)
        s += ",\"iat\":" + String(iat)
        s += ",\"jti\":" + Self.quote(jti)
        s += ",\"revision\":" + String(revision)
        s += "}"
        return Data(s.utf8)
    }

    /// RFC 8785 string escaping.
    public static func quote(_ s: String) -> String {
        var out = "\""
        for scalar in s.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\u{08}": out += "\\b"
            case "\u{0C}": out += "\\f"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default:
                if scalar.value < 0x20 {
                    out += String(format: "\\u%04x", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        return out + "\""
    }
}

/// Compact JWS (RFC 7515) assembly for ES256 decision proofs.
public enum Jws {
    public static let decisionHeader = #"{"alg":"ES256","typ":"loams-decision+jws"}"#

    public static func signingInput(header: String = decisionHeader, payload: Data) -> String {
        Base64URL.encode(Data(header.utf8)) + "." + Base64URL.encode(payload)
    }

    /// `rawSignature` is r||s, 64 bytes (`P256.Signing.ECDSASignature.rawRepresentation`).
    public static func compact(signingInput: String, rawSignature: Data) -> String {
        signingInput + "." + Base64URL.encode(rawSignature)
    }
}

/// Why a decision cannot be sent as entered (AP3 Task 6, same rules as Android).
public enum DecisionProblem: Sendable, Equatable {
    case reasonRequired
    case confirmationMismatch
    case offline
    case notPending
}

public enum DecisionRules {
    public static func check(decision: Decision, pending: Bool, destructive: Bool, confirmText: String, typed: String, reason: String, live: Bool) -> DecisionProblem? {
        if !pending { return .notPending }
        if !live { return .offline }
        if decision == .reject && reason.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return .reasonRequired }
        if decision == .approve && destructive && (confirmText.isEmpty || typed.trimmingCharacters(in: .whitespacesAndNewlines) != confirmText) {
            return .confirmationMismatch
        }
        return nil
    }
}
