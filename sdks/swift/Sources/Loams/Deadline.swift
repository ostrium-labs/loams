// Client deadlines (runtime contract R12, SDKP §2.1).
//
// A unary call has one budget, 30 s unless the client or the call names another,
// covering every attempt and every backoff. Each attempt tells the server what is
// left (`connect-timeout-ms`). When the budget runs out the call fails with
// `deadline_exceeded`, reason `client_deadline_exceeded`, and is not retried.

import Foundation

/// The unary deadline when neither the client nor the call names one, in seconds.
public let loamsDefaultTimeout: Double = 30

/// The client-side reason a spent budget carries (docs/api/reasons.md, client-side).
public let loamsClientDeadlineExceeded = "client_deadline_exceeded"

/// The error a spent budget raises.
func loamsDeadlineExceeded(_ rpc: String, budget: Double) -> LoamsError {
    let ms = Int((budget * 1000).rounded())
    return LoamsError.loams(
        code: .deadlineExceeded,
        reason: .deadlineExceeded,
        unknownReason: loamsClientDeadlineExceeded,
        metadata: ["timeout_ms": String(ms)],
        hint: "",
        rpc: rpc,
        message: "\(rpc): the call did not finish in \(ms) ms"
    )
}

/// The packages R4's session applies to: collection reads accept a merged token.
let loamsSessionPackages: Set<String> = ["loams.collection.v1", "loams.sql.v1"]
