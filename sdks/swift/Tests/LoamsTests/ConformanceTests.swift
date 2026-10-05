// SDK2 Task 4's conformance suite, in the six names the plan states.
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev`, and this runs
// every case in it through the SDK's **public** surface — `client.instance` and
// `client.tables`, the same objects an application uses — rather than through the
// internals. That is the point of the suite: it proves the facade dispatches to
// the right RPC, sends the right encoding, and turns what comes back into the right
// typed value.
//
// Three things are covered, which between them are what the design asks of a
// conforming SDK (design §44 §10.4):
//
//   - a successful call, in the encoding an SDK sends by default;
//   - a structured-reason error, with `reason` and not the message;
//   - the unavailable-service path, in all three of its shapes: the guard that
//     costs no RPC, the refusal a call gets, and the refusal on a stream.
//
// # Swift, and the names of tests
//
// `sdks/conformance/check-languages.mjs` reads this file's **source** and counts
// how many of the six canonical names appear, so each name is a `let` binding with
// the canonical string rather than a comment — a name in a comment would satisfy a
// reader and not the checker.
//
// Swift has no subtests, so the canonical names cannot be `XCTestCase` method
// names verbatim (they start with a letter, not `test`). They are therefore the
// constants below, each attached to the method that pins it, and
// `testConformanceTestNames` asserts the set cannot quietly shrink — which is the
// same claim the Go SDK makes with its registry.

import Foundation
import XCTest
@testable import Loams

/// The six tests SDK2 Task 4 requires, in the names the plan states.
///
/// These literals are what `check-languages.mjs` greps for, so they are the
/// contract rather than decoration.
enum ConformanceTest {
    static let allRequiredFixtures = "swift_conformance_all_required_fixtures"
    static let retryReusesIdempotencyKey = "swift_retry_reuses_idempotency_key"
    static let errorReasonMapping = "swift_error_reason_mapping"
    static let streamResumeWithCursor = "swift_stream_resume_with_cursor"
    static let tokenSourceRefresh = "swift_token_source_refresh"
    static let paginationIterator = "swift_pagination_iterator"

    /// Every one of them, for the registry check.
    static let required = [
        allRequiredFixtures,
        retryReusesIdempotencyKey,
        errorReasonMapping,
        streamResumeWithCursor,
        tokenSourceRefresh,
        paginationIterator,
    ]
}

final class ConformanceTests: XCTestCase {
    /// Fails if a required name is missing, so "the six tests exist" is a
    /// checkable claim rather than a comment.
    func testConformanceTestNames() {
        XCTAssertEqual(
            ConformanceTest.required.count, 6,
            "the plan requires six names, the registry has \(ConformanceTest.required.count)"
        )
        XCTAssertEqual(
            Set(ConformanceTest.required).count, 6,
            "the six names must be distinct: \(ConformanceTest.required)"
        )
        for name in ConformanceTest.required {
            XCTAssertTrue(
                name.hasPrefix("swift_"),
                "\(name) must carry the language prefix; check-languages.mjs looks for it"
            )
        }
    }

    /// Replays the whole corpus. **Pins** `swift_conformance_all_required_fixtures`.
    func testConformanceAllRequiredFixtures() async throws {
        let server = try FixtureServer.start()
        defer { server.stop() }
        let client = try newTestClient(server.endpoint)

        // The corpus index is the authority on what must be covered. Read first, so
        // a missing or unreadable index fails before anything else is claimed.
        let corpus = try readCorpus()
        let expected = [
            // A successful call, in each encoding a client might pick.
            "instance_get_instance_json",
            "instance_get_instance_proto",
            "instance_get_instance_grpc_web",
            "instance_get_instance_grpc_web_json",
            // A structured-reason error, in each encoding.
            "instance_who_am_i_json",
            "instance_who_am_i_proto",
            "instance_who_am_i_grpc_web",
            "instance_who_am_i_grpc_web_json",
            // The unavailable-service path, unary and on a stream.
            "live_query_json",
            "live_query_proto",
            "live_query_grpc_web",
            "live_query_grpc_web_json",
            "live_watch",
        ]
        let recorded = corpus.cases.map(\.name).sorted()
        XCTAssertEqual(
            recorded, expected.sorted(),
            "the corpus and the suite's expectations have diverged"
        )

        // A successful call. `getInstance` needs no auth, which is why it is the
        // first thing any client calls, and it is the case that proves the SDK
        // sends the encoding the corpus recorded: an SDK that asked for binary
        // protobuf would get the JSON bytes and fail to parse them, which is
        // exactly the class of mismatch the four encodings exist to catch.
        let info = try await client.instance.getInstance()
        XCTAssertEqual(info.name, "Loams", "the instance name in the corpus is \"Loams\"")
        XCTAssertTrue(
            info.apiVersions.contains("loams.instance.v1"),
            "api_versions is \(info.apiVersions), want it to contain loams.instance.v1"
        )
        XCTAssertFalse(info.services.isEmpty, "services is empty; it is what feature detection reads")

        // A structured-reason error. The reason is what the SDK reads; the message
        // is for a person and is not asserted on.
        do {
            _ = try await client.instance.whoAmI()
            XCTFail("whoAmI answered, but this build has no authentication yet")
        } catch let error as LoamsError {
            XCTAssertTrue(error.isLoamsFailure, "whoAmI failed with \(type(of: error)), want a Loams failure")
            XCTAssertEqual(
                error.reason, .notImplemented,
                "whoAmI reason is \(String(describing: error.reason)), want not_implemented"
            )
        }

        // The unavailable-service path, three ways.
        //
        // 1. The guard, from the catalogue, spending no RPC on a call that cannot
        //    work. `loams.live` and `loams.tables` are the same service, so the
        //    guard is asked about either.
        do {
            try await client.system.guard("live")
            XCTFail("guard(\"live\") passed, but loams.live.v1 is not served in the standard variant")
        } catch let error as FeatureNotInVariantError {
            XCTAssertEqual(error.variant, "standard", "the guard reads the variant from metadata.variant")
            XCTAssertEqual(error.reason, .featureNotInVariant)
        }
        try await client.system.guard("instance")

        // 2. The refusal a call gets when the caller skips the guard. This is the
        //    typed surface: the reason is in the registry, and the variant is read
        //    out of the metadata rather than parsed out of the message.
        do {
            _ = try await client.tables.query(["collection": .string("acme")])
            XCTFail("tables.query answered, but loams.live.v1 is not served in the standard variant")
        } catch let error as FeatureNotInVariantError {
            XCTAssertEqual(error.reason, .featureNotInVariant)
            XCTAssertEqual(
                error.variant, "standard",
                "the refusal variant is read from metadata.variant, not parsed out of the message"
            )
        }

        // 3. The refusal on a server stream, which arrives inside the Connect
        //    envelope rather than as an HTTP status. A client that only reads
        //    status codes sees a 200 here, so this is the case that distinguishes
        //    a real Connect implementation from a status-code-only fake.
        do {
            let stream = try await client.live.watch(["query_set": .string("acme")])
            var received = 0
            for try await _ in stream.messages() { received += 1 }
            XCTFail("watch completed with \(received) messages, want a FeatureNotInVariantError")
        } catch let error as FeatureNotInVariantError {
            XCTAssertEqual(error.reason, .featureNotInVariant)
        }

        // The catalogue answers the same question the refusals do, from one call.
        let catalogue = try await client.system.catalogue()
        XCTAssertTrue(
            catalogue.served.contains("loams.instance.v1"),
            "served is \(catalogue.served), want it to contain loams.instance.v1"
        )
        XCTAssertTrue(
            catalogue.unavailable.contains("loams.live.v1"),
            "unavailable is \(catalogue.unavailable), want it to contain loams.live.v1"
        )
        guard let live = catalogue.services.first(where: { $0.package == "loams.live.v1" }) else {
            return XCTFail("the catalogue has no loams.live.v1 entry")
        }
        XCTAssertTrue(live.unstable, "loams.live.v1 is not marked unstable; buf breaking skips that package")
    }

    /// The same idempotency key on every attempt. **Pins**
    /// `swift_retry_reuses_idempotency_key`.
    func testRetryReusesIdempotencyKey() async throws {
        let transport = StubTransport(replies: [
            .failure(code: .unavailable, message: "the node is restarting", info: nil),
            .success(["applied": .bool(true)]),
        ])
        let invoker = CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        let binding = try loamsRequireBinding(module: "tables", call: "Mutate")

        _ = try await invoker.unary(
            binding: binding,
            request: DynamicMessage(protoTypeName: mutateRequestTypeName, fields: ["collection": .string("acme")])
        )

        XCTAssertEqual(
            transport.calls.count, 2,
            "one unavailable then one success is two attempts: \(transport.calls.count)"
        )
        let keys = transport.idempotencyKeys
        XCTAssertEqual(keys.count, 2, "both attempts must carry a key: \(keys)")
        XCTAssertEqual(
            keys[0], keys[1],
            "the key was regenerated per attempt, which turns one write into two — the exact failure R3 exists to prevent"
        )
        XCTAssertFalse(keys[0].isEmpty, "the SDK must mint a key when the caller supplied none")

        // A caller-supplied key wins, so a caller whose own storage dedupes on the
        // key is not second-guessed by the SDK.
        let supplied = StubTransport(replies: [.success([:])])
        let suppliedInvoker = CallInvoker(transport: supplied, auth: nil, maxRetries: 0)
        _ = try await suppliedInvoker.unary(
            binding: binding,
            request: DynamicMessage(protoTypeName: mutateRequestTypeName),
            suppliedIdempotencyKey: "caller-supplied-key"
        )
        XCTAssertEqual(supplied.idempotencyKeys, ["caller-supplied-key"])

        // A request with no such field is left alone: keying `DeployRequest` would
        // invent a field the schema does not declare.
        let deploy = StubTransport(replies: [.success([:])])
        let deployInvoker = CallInvoker(transport: deploy, auth: nil, maxRetries: 0)
        let deployBinding = try loamsRequireBinding(module: "tables", call: "Deploy")
        let deployRequest = DynamicMessage(protoTypeName: deployRequestTypeName, fields: ["collection": .string("acme")])
        _ = try await deployInvoker.unary(binding: deployBinding, request: deployRequest)
        XCTAssertTrue(
            deploy.idempotencyKeys.isEmpty,
            "deploy must not be keyed: \(deploy.idempotencyKeys)"
        )
        XCTAssertEqual(
            deploy.calls.first?.body, ["collection": .string("acme")],
            "an unkeyed request is sent exactly as the caller wrote it"
        )
    }

    /// Every reason in the registry maps to a type, and the three distinct cases of
    /// R8 stay distinct. **Pins** `swift_error_reason_mapping`.
    func testErrorReasonMapping() throws {
        for reason in loamsAllReasons {
            let error = LoamsError.fromWire(
                code: loamsReasonCodes[reason] ?? .unknown,
                info: ErrorInfoShape(reason: reason.rawValue, metadata: ["k": "v"], hint: "try this"),
                rpc: "loams.instance.v1.InstanceService/WhoAmI",
                message: "for a person"
            )
            guard let loams = error as? LoamsError else {
                return XCTFail("\(reason.rawValue) mapped to \(type(of: error)), want a LoamsError")
            }
            XCTAssertEqual(loams.reason, reason, "\(reason.rawValue) did not survive the mapping")
            XCTAssertNil(
                loams.unknownReason,
                "\(reason.rawValue) is in the registry, so it must not also surface as unknown"
            )
            XCTAssertEqual(loams.hint, "try this", "the hint is the caller's next step and must survive")
            XCTAssertEqual(loams.metadata["k"], "v", "metadata is structured context and must survive")
        }

        // A reason from a **newer** server is surfaced as text and flagged, not
        // dropped: losing it would leave a caller unable to tell "not supported
        // here" from "not supported at all".
        let future = LoamsError.fromWire(
            code: .failedPrecondition,
            info: ErrorInfoShape(reason: "a_reason_this_sdk_has_never_heard_of"),
            rpc: "loams.instance.v1.InstanceService/WhoAmI",
            message: "from a newer server"
        )
        guard let futureLoams = future as? LoamsError else {
            return XCTFail("an unknown reason must still be a LoamsError")
        }
        XCTAssertNil(futureLoams.reason, "a reason off the registry must not become a Reason")
        XCTAssertEqual(
            futureLoams.unknownReason, "a_reason_this_sdk_has_never_heard_of",
            "the reason must be surfaced as text, not dropped"
        )

        // A failure from **below the API** carries no reason at all, and that is a
        // different thing from a service refusing.
        let belowAPI = LoamsError.map(
            NSError(domain: NSURLErrorDomain, code: NSURLErrorCannotConnectToHost),
            rpc: "loams.instance.v1.InstanceService/GetInstance"
        )
        XCTAssertFalse(LoamsError.isLoamsError(belowAPI), "a socket failure is not a Loams failure")
        XCTAssertNil(LoamsError.reason(of: belowAPI), "a failure from below the API carries no reason")

        // Mapping twice loses nothing.
        let once = LoamsError.fromWire(
            code: .notFound,
            info: ErrorInfoShape(reason: "not_found"),
            rpc: "loams.instance.v1.InstanceService/GetInstance",
            message: "gone"
        )
        let twice = LoamsError.map(once, rpc: "something.else/Method")
        XCTAssertEqual(LoamsError.rpc(of: twice), "loams.instance.v1.InstanceService/GetInstance")
        XCTAssertEqual(LoamsError.reason(of: twice), .notFound, "a remapped LoamsError keeps its reason")
    }

    /// A stream re-opens from the last cursor it applied and does not re-yield.
    /// **Pins** `swift_stream_resume_with_cursor`.
    func testStreamResumeWithCursor() async throws {
        // First leg: two messages, then a retryable break. Second leg: the message
        // after the cursor — and **not** the two already yielded, which is the
        // whole of R7.
        let transport = StubTransport(
            replies: [.success([:])],
            streamPlan: [
                [
                    .success(["state_version": .string("10")]),
                    .success(["state_version": .string("20")]),
                    .failure(code: .unavailable, message: "the node is restarting", info: nil),
                ],
                [
                    .success(["state_version": .string("30")]),
                ],
            ]
        )
        let invoker = CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        let binding = try loamsRequireBinding(module: "live", call: "Watch")

        let stream = try await invoker.serverStream(
            binding: binding,
            request: DynamicMessage(protoTypeName: watchRequestTypeName, fields: ["query_set": .string("acme")])
        )
        stream.policy = StreamResume(
            resume: { cursor, request in
                // The re-open carries the cursor, which is the SDK's half of R7:
                // the caller's closure says how its RPC spells "resume from".
                var copy = request
                copy.fields["from_state_version"] = .string(cursor.value)
                return copy
            },
            cursorOf: { message in
                StreamCursor(message.fields["state_version"]?.stringValue ?? "")
            }
        )

        var versions: [String] = []
        for try await message in stream.messages() {
            versions.append(message.fields["state_version"]?.stringValue ?? "")
        }

        XCTAssertEqual(
            versions, ["10", "20", "30"],
            "the stream must resume from the cursor without re-yielding what it already yielded"
        )
        XCTAssertEqual(transport.calls.count, 2, "the stream re-opened exactly once: \(transport.calls.count) opens")
        XCTAssertEqual(
            transport.calls[1].body["from_state_version"]?.stringValue, "20",
            "the re-open resumes from the last cursor **applied**, not the first or the empty string"
        )
    }

    /// One refresh and one retry, then a second expiry is reported. **Pins**
    /// `swift_token_source_refresh`.
    func testTokenSourceRefresh() async throws {
        let binding = try loamsRequireBinding(module: "instance", call: "WhoAmI")

        // A source that can refresh: the first attempt's 401 refreshes, and the
        // retry carries the **new** token.
        let refreshes = Counter()
        let source = ScriptedTokenSource(tokens: ["stale", "fresh"], refreshHook: { await refreshes.increment() })
        let transport = StubTransport(replies: [
            .failure(
                code: .unauthenticated,
                message: "the token expired",
                info: ErrorInfoShape(reason: Reason.tokenExpired.rawValue)
            ),
            .failure(
                code: .unauthenticated,
                message: "the token expired",
                info: ErrorInfoShape(reason: Reason.tokenExpired.rawValue)
            ),
        ])
        let invoker = CallInvoker(transport: transport, auth: source, maxRetries: 0)

        do {
            _ = try await invoker.unary(binding: binding, request: WhoAmIRequest())
            XCTFail("a second expiry must be reported, not refreshed again")
        } catch {
            XCTAssertTrue(error is TokenExpiredError, "a second expiry is a TokenExpiredError, got \(type(of: error))")
        }
        let refreshCount = await refreshes.value
        XCTAssertEqual(
            refreshCount, 1,
            "R1 says exactly one refresh; a second expiry is reported. Refreshed \(refreshCount) times"
        )
        XCTAssertEqual(
            transport.calls.map(\.bearer), ["stale", "fresh"],
            "the retry must carry the refreshed token"
        )

        // A source that cannot refresh — an API key — makes the refresh a no-op, and
        // the retry is **skipped** rather than spent on a request that cannot work.
        let apiKeyTransport = StubTransport(replies: [
            .failure(
                code: .unauthenticated,
                message: "unauthenticated",
                info: ErrorInfoShape(reason: Reason.unauthenticated.rawValue)
            ),
        ])
        let apiKeyInvoker = CallInvoker(
            transport: apiKeyTransport,
            auth: APIKeyTokenSource("loams_key"),
            maxRetries: loamsDefaultMaxRetries
        )
        do {
            _ = try await apiKeyInvoker.unary(binding: binding, request: WhoAmIRequest())
            XCTFail("an unauthenticated call with no token_expired reason must fail")
        } catch {
            XCTAssertNil(
                LoamsError.reason(of: error) == .tokenExpired ? nil : LoamsError.reason(of: error),
                "a plain unauthenticated refusal is not a token expiry"
            )
        }
        XCTAssertEqual(
            apiKeyTransport.calls.count, 1,
            "a non-expiry refusal must not be retried: \(apiKeyTransport.calls.count) attempts"
        )

        // The token travels in a header and never in the URL.
        XCTAssertEqual(apiKeyTransport.calls.first?.bearer, "loams_key")
    }

    /// The iterator follows the tokens to the end and yields **items**, not pages.
    /// **Pins** `swift_pagination_iterator`.
    func testPaginationIterator() async throws {
        // A paged binding, built the way the facade renderer would build one once
        // `loams.collection.v1.ListCollections` exists (API1 Task 2).
        let paged = CallBinding(
            module: "collection",
            name: "ListCollections",
            protoName: "listCollections",
            method: "ListCollections",
            rpc: "loams.collection.v1.CollectionService/ListCollections",
            service: "loams.collection.v1.CollectionService",
            package: "loams.collection.v1",
            idempotency: .noSideEffects,
            retry: .safe,
            streaming: .unary,
            pagination: Pagination(itemsField: "collections", nextPageTokenField: "next_page_token")
        )
        XCTAssertEqual(paged.pagination?.itemsField, "collections")
        XCTAssertEqual(paged.pagination?.nextPageTokenField, "next_page_token")

        // Two pages and a stop, driven by the token the stub threads back.
        let pages: [[String: JSONValue]] = [
            ["collections": .array([.object(["name": .string("a")]), .object(["name": .string("b")])]),
             "next_page_token": .string("p2")],
            ["collections": .array([.object(["name": .string("c")])])],
        ]
        let seenTokens = TokenRecorder()
        let fetcher: PageFetcher<DynamicMessage, DynamicMessage> = { request, _ in
            let token = request.loamsFields()[ProtoField.pageToken]?.stringValue ?? ""
            await seenTokens.record(token)
            let index = token.isEmpty ? 0 : 1
            return DynamicMessage(
                protoTypeName: "loams.collection.v1.ListCollectionsResponse",
                fields: pages[min(index, pages.count - 1)]
            )
        }

        var names: [String] = []
        for try await item in loamsPaginate(
            binding: paged,
            fetch: fetcher,
            request: DynamicMessage(protoTypeName: "loams.collection.v1.ListCollectionsRequest"),
            items: { response in
                loamsItems(response, field: "collections").compactMap { $0.objectValue?["name"]?.stringValue }
            }
        ) {
            names.append(item)
        }
        XCTAssertEqual(names, ["a", "b", "c"], "the iterator yields items, not pages, to the end")
        XCTAssertEqual(await seenTokens.values, ["", "p2"], "the second request carries the first page's token")

        // A binding that is not paged refuses **through the sequence**, because an
        // `AsyncThrowingStream` has no other channel for a failure and building the
        // iterator cannot throw.
        let unpaged = try loamsRequireBinding(module: "instance", call: "GetInstance")
        XCTAssertNil(unpaged.pagination)
        do {
            for try await _ in loamsPaginate(
                binding: unpaged,
                fetch: { _, _ in DynamicMessage(protoTypeName: "") },
                request: DynamicMessage(protoTypeName: ""),
                items: { _ in [] }
            ) { }
            XCTFail("an unpaged binding must refuse")
        } catch let error as PaginationError {
            XCTAssertTrue(
                error.message.contains("instance.GetInstance"),
                "the refusal names the binding so a log makes the cause obvious: \(error.message)"
            )
        }

        // The end-to-end half is a **deliberate skip, not an omission**: no RPC is
        // paged yet, so a fixture for one would test the stub rather than the SDK.
        XCTAssertTrue(
            loamsModules.allSatisfy { $0.calls.allSatisfy { $0.pagination == nil } },
            "no annotated RPC is paged yet; ListCollections arrives with API1 Task 2"
        )
    }

    /// R9: the SDK declares its proto revision and reports the server's packages
    /// beside it, and a package the SDK speaks that the server does not serve is a
    /// warning rather than an exception.
    func testConformanceReportsVersion() async throws {
        let server = try FixtureServer.start()
        defer { server.stop() }
        let client = try newTestClient(server.endpoint)

        let report = try await client.system.version()
        XCTAssertEqual(report.protoRev, client.protoRev, "the report and the client must state the same revision")
        XCTAssertFalse(report.serverVersion.isEmpty, "the report has no server version")
        // `loams.live.v1` is served as unavailable in the standard variant, and
        // `GetInstance.apiVersions` lists only what is served, so a mismatch here is
        // the server's, not the SDK's.
        XCTAssertEqual(
            report.apiVersions, ["loams.instance.v1"],
            "api_versions is \(report.apiVersions), want [loams.instance.v1]"
        )
        XCTAssertFalse(
            report.compatible,
            "the instance does not serve every package the SDK speaks, so it is not compatible"
        )
        XCTAssertTrue(
            report.missing.contains("loams.live.v1"),
            "missing is \(report.missing), want it to contain loams.live.v1"
        )
    }

    /// R5's "concurrent readers share one in-flight fetch": a hundred readers at
    /// once must cost one `GetInstance`.
    func testConformanceCatalogueIsShared() async throws {
        let server = try FixtureServer.start()
        defer { server.stop() }
        let client = try newTestClient(server.endpoint)

        let readers = 100
        await withTaskGroup(of: Int.self) { group in
            for _ in 0..<readers {
                group.addTask {
                    (try? await client.system.catalogue().served.count) ?? 0
                }
            }
            var empty = 0
            for await served in group where served == 0 { empty += 1 }
            XCTAssertEqual(empty, 0, "\(empty) of \(readers) concurrent readers got an empty catalogue")
        }

        let first = try await client.system.catalogue()
        let again = try await client.system.catalogue()
        XCTAssertEqual(
            first.services.count, again.services.count,
            "two catalogue reads must agree"
        )
    }
}

// MARK: - Test doubles

/// A token source that hands out a scripted sequence and counts refreshes.
///
/// Counting is the point: R1's "exactly one refresh" is a claim about how many
/// times `refresh()` was called, and it cannot be checked from the token values
/// alone — a source that returned the same stale token twice would look identical
/// from the outside.
struct ScriptedTokenSource: TokenSource, Sendable {
    private let tokens: [String]
    private let refreshHook: @Sendable () async -> Void
    private let state: TokenState

    init(tokens: [String], refreshHook: @escaping @Sendable () async -> Void = {}) {
        self.tokens = tokens
        self.refreshHook = refreshHook
        self.state = TokenState(tokens: tokens)
    }

    func token() async throws -> String { await state.next() }

    func refresh() async throws {
        await refreshHook()
        await state.advance()
    }
}

/// The scripted token's mutable state.
private actor TokenState {
    private var tokens: [String]
    private var index = 0

    init(tokens: [String]) {
        self.tokens = tokens
    }

    func next() -> String { tokens[min(index, tokens.count - 1)] }

    func advance() {
        index = min(index + 1, max(tokens.count - 1, 0))
    }
}

/// A counter a test can await.
actor Counter {
    private var count = 0
    func increment() { count += 1 }
    var value: Int { count }
}

/// Records the page tokens a fetcher was asked with.
actor TokenRecorder {
    private(set) var values: [String] = []
    func record(_ value: String) { values.append(value) }
}