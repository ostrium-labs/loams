// The module surface: one method per facade binding.
//
// This is the layer D606 says is generated. **For Swift it is the Q604
// hand-written fallback** (see `Facade.swift`'s header), and it is the file the
// facade renderer would replace: every method below resolves its binding from
// ``loamsBinding(module:call:)`` and hands it to the one ``CallInvoker``, so there
// is no RPC path and no retry class in this file. That is what makes annotating a
// proto enough to add an SDK method, and it is why ``ConformanceTests`` can
// assert that every binding in the catalogue has a method by checking that
// resolving each one succeeds.

import Foundation

/// Resolves a binding or fails with a message naming the module and the call.
///
/// The one place a missing binding is reported. A caller that asks for a call the
// catalogue does not have gets an `internal` failure naming both — which is a bug
/// in the facade, not in the caller's code, so `internal` is the honest code
/// (D611).
func loamsRequireBinding(module: String, call: String) throws -> CallBinding {
    guard let binding = loamsBinding(module: module, call: call) else {
        throw LoamsError.internalError(
            "",
            "loams: \(module).\(call) has no generated binding; the proto is missing the "
                + "loams.options.v1 annotation (Q604)"
        )
    }
    return binding
}

/// `loams.instance` — what this instance is, and who the caller is on it.
public struct InstanceModule: Sendable {
    let invoker: CallInvoker

    public init(invoker: CallInvoker) {
        self.invoker = invoker
    }

    /// What this instance is and which packages it serves.
    ///
    /// This is the first thing any client calls: it needs no credential, and its
    /// `services[]` is what feature detection reads (R5).
    public func getInstance(options: CallOptions = .init()) async throws -> GetInstanceResponse {
        let binding = try loamsRequireBinding(module: "instance", call: "GetInstance")
        return try await invoker.unary(
            binding: binding,
            request: GetInstanceRequest(),
            overrides: options
        )
    }

    /// Who the caller is on this instance.
    ///
    /// **This build has no authentication yet**, so every variant refuses with
    /// `reason = not_implemented`. That refusal is the recorded corpus's
    /// `instance_who_am_i_*` case and is what R8's reason mapping is pinned
    /// against.
    public func whoAmI(options: CallOptions = .init()) async throws -> WhoAmIResponse {
        let binding = try loamsRequireBinding(module: "instance", call: "WhoAmI")
        return try await invoker.unary(
            binding: binding,
            request: WhoAmIRequest(),
            overrides: options
        )
    }
}

/// `loams.live` — the live sync session half.
///
/// Its package is unstable, so its wire contract may still change (§44 §10.3).
/// `loams.live.v1` is **not served** in the standard variant, so every method here
/// answers `unimplemented` with `reason = feature_not_in_variant` — which is the
/// corpus's `live_query_*` and `live_watch` cases.
public struct LiveModule: Sendable {
    let invoker: CallInvoker

    public init(invoker: CallInvoker) {
        self.invoker = invoker
    }

    /// Changes the query set a watch is over.
    ///
    /// A mutation: retryable only once it carries an idempotency key, and its
    /// request has no `idempotency_key` field, so it is never keyed (R3).
    public func modifyQuerySet(
        _ request: DynamicMessage,
        options: CallOptions = .init()
    ) async throws -> DynamicMessage {
        let binding = try loamsRequireBinding(module: "live", call: "ModifyQuerySet")
        return try await invoker.unary(binding: binding, request: request, overrides: options)
    }

    /// Watches a query set over a server stream.
    ///
    /// The result is an `AsyncSequence` (R7). Passing a `resume` turns on
    /// re-opening from the last applied cursor; without one, a broken stream is
    /// **reported** rather than spun on.
    ///
    ///     for try await transition in client.live.watch(request, resume: resume) {
    ///         apply(transition)
    ///     }
    public func watch(
        _ request: DynamicMessage,
        resume: StreamResume<DynamicMessage, DynamicMessage>? = nil
    ) async throws -> ResumableServerStream<DynamicMessage, DynamicMessage> {
        let binding = try loamsRequireBinding(module: "live", call: "Watch")
        var stream = try await invoker.serverStream(binding: binding, request: request)
        stream.policy = resume
        return stream
    }
}

/// `loams.tables` — the table half of `loams.live.v1` (design §44 §7.2).
///
/// A second facade name for the same RPCs, which is why
/// ``LoamsSystem/availableModule(_:)`` answers the same for `tables` and `live`.
public struct TablesModule: Sendable {
    let invoker: CallInvoker

    public init(invoker: CallInvoker) {
        self.invoker = invoker
    }

    /// Queries the live tables.
    public func query(_ request: DynamicMessage, options: CallOptions = .init()) async throws -> DynamicMessage {
        let binding = try loamsRequireBinding(module: "tables", call: "Query")
        return try await invoker.unary(binding: binding, request: request, overrides: options)
    }

    /// Mutates the live tables.
    ///
    /// The one call whose request declares `idempotency_key`
    /// (``CallBinding/takesIdempotencyKey``), so R3 mints one before the first
    /// attempt and reuses it on every retry — which is what makes this the SDK's
    /// one retryable mutation.
    public func mutate(_ request: DynamicMessage, options: CallOptions = .init()) async throws -> DynamicMessage {
        let binding = try loamsRequireBinding(module: "tables", call: "Mutate")
        return try await invoker.unary(
            binding: binding,
            request: request,
            overrides: options,
            suppliedIdempotencyKey: options.idempotencyKey
        )
    }

    /// Deploys the live tables.
    ///
    /// Its request has **no** `idempotency_key` field, so R3 leaves it alone and it
    /// is never retried. Keying it would invent a field the schema does not have.
    public func deploy(_ request: DynamicMessage, options: CallOptions = .init()) async throws -> DynamicMessage {
        let binding = try loamsRequireBinding(module: "tables", call: "Deploy")
        return try await invoker.unary(binding: binding, request: request, overrides: options)
    }
}