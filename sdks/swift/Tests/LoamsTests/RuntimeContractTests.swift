// Unit tests for the clauses the runtime contract implies beyond the six named
// conformance tests.
//
// Each block names the clause it pins. Where `docs/sdk/runtime-contract.md` says a
// clause is *not* pinned by a test anywhere, that is said here too, because a gap
// recorded in a test file is a gap someone will close; a gap that only exists in a
// design document's prose is a gap that gets rediscovered.

import Foundation
import XCTest
@testable import Loams

// MARK: - R2 — retry classes and backoff

final class RetryContractTests: XCTestCase {
    /// The numbers are M1.6 Ruling 5's and are the same in every SDK. If these
    /// change, every SDK's changes, and that is a decision, not a fix.
    func testBackoffNumbersAreTheRulings() {
        XCTAssertEqual(loamsBaseDelayMilliseconds, 100, "base is 100 ms")
        XCTAssertEqual(loamsMaxDelayMilliseconds, 2000, "the cap is 2 s")
        XCTAssertEqual(loamsDefaultMaxRetries, 3, "three retries after the first attempt")
        XCTAssertEqual(loamsMaxServerDelayMilliseconds, 30_000, "a server-sent delay is capped at 30 s")
    }

    /// Full jitter: uniform over `[0, min(cap, base × 2^attempt)]`.
    ///
    /// Checked as a **bound** rather than a distribution, because a distribution
    /// test with a random source is flaky by construction and a flaky test is a
    /// test that will be disabled. The upper bound is the contract; the uniformity
    /// is documented.
    func testBackoffIsFullJitterWithinItsCeiling() {
        for attempt in 0..<8 {
            let ceiling = min(loamsMaxDelayMilliseconds, loamsBaseDelayMilliseconds << attempt)
            var lowest = Int.max
            var highest = 0
            for _ in 0..<200 {
                let value = loamsBackoff(attempt: attempt)
                XCTAssertGreaterThanOrEqual(value, 0, "jitter is over [0, ceiling], never negative")
                XCTAssertLessThanOrEqual(value, ceiling, "attempt \(attempt) exceeded base × 2^attempt")
                lowest = min(lowest, value)
                highest = max(highest, value)
            }
            // Full jitter means the range is actually **used**. An implementation
            // that returned the ceiling every time would pass a bounds-only check
            // and defeat the clause, so the spread is asserted too: over 200 draws
            // the minimum must come in below the ceiling.
            XCTAssertLessThan(
                lowest, ceiling,
                "attempt \(attempt) never jittered below its ceiling, so this is not full jitter"
            )
            _ = highest
        }
    }

    /// A large `attempt` must not overflow into a negative ceiling, which would
    /// trap in `Int.random`. A caller passing `attempt = 1000` gets the cap.
    func testBackoffDoesNotOverflowOnAHugeAttempt() {
        for attempt in [16, 31, 62, 64, 1000, Int.max] {
            let value = loamsBackoff(attempt: attempt)
            XCTAssertGreaterThanOrEqual(value, 0, "attempt \(attempt) produced a negative backoff")
            XCTAssertLessThanOrEqual(value, loamsMaxDelayMilliseconds)
        }
    }

    /// A server-sent `RetryInfo.retry_delay` replaces the computed backoff, up to
    /// 30 s. Nothing in the protos carries `RetryInfo` yet (R2), so this pins the
    /// hook rather than the wire.
    func testServerDelayReplacesTheComputedBackoff() {
        XCTAssertEqual(loamsBackoff(attempt: 0, serverDelayMilliseconds: 750), 750)
        XCTAssertEqual(
            loamsBackoff(attempt: 0, serverDelayMilliseconds: 90_000),
            loamsMaxServerDelayMilliseconds,
            "a server-sent delay is capped at 30 s"
        )
        XCTAssertEqual(
            loamsBackoff(attempt: 5, serverDelayMilliseconds: 250), 250,
            "the server's delay wins over the computed one"
        )
    }

    /// The retryable set is exactly the three D610 names.
    func testRetryableCodes() {
        for code in [Code.unavailable, .deadlineExceeded, .resourceExhausted] {
            XCTAssertTrue(loamsIsRetryableCode(code), "\(code.rawValue) must be retryable")
        }
        for code in [Code.internal, .unimplemented, .notFound, .unauthenticated, .invalidArgument, .aborted] {
            XCTAssertFalse(loamsIsRetryableCode(code), "\(code.rawValue) must not be retryable")
        }
    }

    /// A mutation is not retried until it carries a key, and a cancellation is
    /// never retried whatever the class.
    func testShouldRetryRefusals() {
        let unavailable = LoamsError.loams(
            code: .unavailable,
            reason: .unavailable,
            unknownReason: nil,
            metadata: [:],
            hint: "",
            rpc: "pkg.Service/Method",
            message: "restarting"
        )
        XCTAssertTrue(
            loamsShouldRetry(error: unavailable, retrySafe: true, attempt: 0, maxRetries: 3),
            "a read on a retryable code retries"
        )
        XCTAssertFalse(
            loamsShouldRetry(error: unavailable, retrySafe: false, attempt: 0, maxRetries: 3),
            "a mutation without a key does not retry, whatever the code"
        )
        XCTAssertFalse(
            loamsShouldRetry(error: unavailable, retrySafe: true, attempt: 3, maxRetries: 3),
            "the budget bounds the call"
        )
        XCTAssertFalse(
            loamsShouldRetry(error: unavailable, retrySafe: true, attempt: 0, maxRetries: 3, isCancelled: { true }),
            "a cancelled task is never retried, whatever the class"
        )
        XCTAssertFalse(
            loamsShouldRetry(error: LoamsError.cancelled(rpc: "x"), retrySafe: true, attempt: 0, maxRetries: 3),
            "a cancellation carries .canceled, which the retryable set excludes"
        )
    }
}

// MARK: - R3 — idempotency keys

final class IdempotencyContractTests: XCTestCase {
    /// A UUIDv7 has to be unique across every client and sort by creation time.
    func testUUIDv7ShapeAndOrdering() {
        let early = loamsUUIDv7(now: Date(timeIntervalSince1970: 1_000))
        let late = loamsUUIDv7(now: Date(timeIntervalSince1970: 2_000))
        XCTAssertEqual(early.count, 36, "the canonical lowercase hyphenated form is 36 characters")
        // 1_000 s after the epoch is 1_000_000 ms, which is 0x0000000f4240. The
        // prefix asserted here used to be `0190`, which is the timestamp for
        // mid-2024 and could never be produced by this input: it was the expected
        // value for a "now" that had been replaced with a fixed 1_000 s and the
        // expectation left behind. Asserting the arithmetic instead of a magic
        // string is what makes it survive a change of input.
        XCTAssertTrue(
            early.hasPrefix("0000000f"),
            "48 bits of Unix milliseconds is the first 12 hex digits; 1_000 s is 0x0000000f4240"
        )

        // Sorting is the property an operator relies on to correlate keys in a log.
        XCTAssertLessThan(
            early, late,
            "two UUIDv7s must sort by creation time, or a log of them cannot be read"
        )
    }

    /// The version and variant nibbles, because a v7 whose version nibble is wrong
    /// is not a v7 and `loamsUUIDv7Time` will refuse it.
    func testUUIDv7VersionAndVariant() {
        let value = loamsUUIDv7()
        let characters = Array(value)
        XCTAssertEqual(characters[14], "7", "the version nibble must be 7")
        XCTAssertTrue(
            "89ab".contains(characters[19]),
            "the variant nibble must be 8, 9, a or b, got \(characters[19])"
        )
    }

    /// The timestamp is the first **twelve** hex digits. Reading eight returns a
    /// number around 2^25, which is January 1970 — the bug this test exists to
    /// keep fixed.
    func testUUIDv7TimeReadsTwelveDigits() {
        let now = Date(timeIntervalSince1970: 1_700_000_000.123)
        let value = loamsUUIDv7(now: now)
        guard let decoded = loamsUUIDv7Time(value) else {
            return XCTFail("a v7 this SDK minted must decode: \(value)")
        }
        // Within a millisecond: the encoding is milliseconds, so exact equality is
        // not available and an exact assert would be flaky.
        XCTAssertEqual(
            decoded.timeIntervalSince1970, now.timeIntervalSince1970, accuracy: 0.002,
            "the timestamp round-trips"
        )
        XCTAssertGreaterThan(decoded.timeIntervalSince1970, 1_600_000_000, "and is not January 1970")
    }

    /// Anything that is not a v7 is refused rather than half-decoded.
    func testUUIDv7TimeRefusesOtherThings() {
        XCTAssertNil(loamsUUIDv7Time(""))
        XCTAssertNil(loamsUUIDv7Time("not-a-uuid"))
        XCTAssertNil(loamsUUIDv7Time(String(repeating: "a", count: 36)), "the version nibble is not 7")
        // A v4: right length, right hyphens, wrong version.
        let v4 = "0190d3f8-7a1c-4a2b-8c3d-4e5f6a7b8c9d"
        XCTAssertNil(loamsUUIDv7Time(v4), "a v4 is not a v7")
    }

    /// Two keys minted in the same millisecond must still differ, or a retried
    /// write would look like two writes.
    func testUUIDv7IsUniqueWithinAMillisecond() {
        let now = Date(timeIntervalSince1970: 1_700_000_000)
        var seen = Set<String>()
        for _ in 0..<1000 {
            seen.insert(loamsUUIDv7(now: now))
        }
        XCTAssertEqual(seen.count, 1000, "1000 keys in one millisecond must all differ")
    }
}

// MARK: - R4 — consistency tokens

final class ConsistencyContractTests: XCTestCase {
    func testTokenShape() {
        XCTAssertTrue(loamsIsConsistencyToken("v1:abc"))
        XCTAssertFalse(loamsIsConsistencyToken("v1:"), "a prefix with nothing after it is not a token")
        XCTAssertFalse(loamsIsConsistencyToken("v2:abc"), "only v1 exists today")
        XCTAssertFalse(loamsIsConsistencyToken("abc"))
    }

    /// The session keeps the token it was given and reports two different tokens
    /// meeting as an **error** rather than merging them into a wrong one.
    ///
    /// A silently-wrong consistency token reads stale data, which is worse than a
    /// failure — that is the whole reason this is an error.
    func testSessionRefusesToMergeTwoDifferentTokens() async throws {
        let session = ConsistencySession()
        try await session.record("v1:aaa")
        let current = await session.current()
        XCTAssertEqual(current, "v1:aaa")

        do {
            try await session.record("v1:bbb")
            XCTFail("two different tokens must not merge silently")
        } catch {
            XCTAssertEqual(LoamsError.code(of: error), .failedPrecondition)
            XCTAssertTrue(
                "\(error)".contains("cannot merge"),
                "the error must say why: \(error)"
            )
        }
        // The first token survives: the session keeps what it had rather than
        // adopting one of two candidates.
        let kept = await session.current()
        XCTAssertEqual(kept, "v1:aaa", "the session keeps the first token")
        let conflicts = await session.conflicts()
        XCTAssertEqual(conflicts, 1, "the unmergeable pair is counted, not silent")
    }

    /// The same token twice is not a conflict, and an empty token is not a token.
    func testSessionIdempotenceAndEmptiness() async throws {
        let session = ConsistencySession()
        try await session.record("v1:aaa")
        try await session.record("v1:aaa")
        let conflicts = await session.conflicts()
        XCTAssertEqual(conflicts, 0, "the same token twice is not a conflict")

        try await session.record("")
        let afterEmpty = await session.current()
        XCTAssertEqual(afterEmpty, "v1:aaa", "an empty token is not a token")
    }

    func testSessionClears() async {
        let session = ConsistencySession()
        try? await session.record("v1:aaa")
        await session.clear()
        let current = await session.current()
        XCTAssertNil(current, "clear forgets the token")
        let conflicts = await session.conflicts()
        XCTAssertEqual(conflicts, 0, "and the conflict count with it")
    }
}

// MARK: - R1 — token sources

final class TokenSourceContractTests: XCTestCase {
    /// A source whose cache starts empty would send no credential, and an instance
    /// that requires one answers `unauthenticated` — which the call path treats as
    /// "the token expired" and retries, with still no credential. So the first
    /// `token()` fetches.
    func testRefreshingSourceFetchesOnFirstUse() async throws {
        let fetches = Counter()
        let source = RefreshingTokenSource {
            await fetches.increment()
            return "minted"
        }
        let token = try await source.token()
        XCTAssertEqual(token, "minted")
        let count = await fetches.value
        XCTAssertEqual(count, 1)

        // Cached: a second read must not re-fetch.
        let again = try await source.token()
        XCTAssertEqual(again, "minted")
        let afterSecond = await fetches.value
        XCTAssertEqual(afterSecond, 1, "the second read is served from the cache")
    }

    /// One in-flight refresh shared across concurrent callers, so a burst of `401`s
    /// produces **one** token exchange rather than one per request.
    ///
    /// This is not a micro-optimisation: an instance rejecting every token because
    /// it is stale would otherwise be hit with one exchange per in-flight call,
    /// which is how a credential rotation becomes a self-inflicted denial of
    /// service.
    func testConcurrentRefreshesShareOneFetch() async throws {
        let fetches = Counter()
        let source = RefreshingTokenSource {
            await fetches.increment()
            // Long enough that every task is waiting on the same in-flight fetch,
            // which is the condition being tested.
            try? await Task.sleep(nanoseconds: 20_000_000)
            return "shared"
        }

        let tokens = await withTaskGroup(of: String?.self, returning: [String?].self) { group in
            for _ in 0..<20 {
                group.addTask { try? await source.token() }
            }
            var out: [String?] = []
            for await token in group { out.append(token) }
            return out
        }

        let count = await fetches.value
        XCTAssertEqual(count, 1, "20 concurrent readers must cost one fetch, not 20")
        XCTAssertEqual(tokens.compactMap { $0 }.count, 20, "every reader must get a token")
        XCTAssertEqual(Set(tokens.compactMap { $0 }), ["shared"])
    }

    /// The environment is read on **every** call, so a process that receives its
    /// credentials after the source is built still authenticates.
    func testEnvironmentIsReadEveryCall() async throws {
        // A lock, not a captured `var`: the lookup is `@Sendable` and this test
        // is `async`, so a captured mutable local would be shared across
        // concurrent executions — which is the thing this test exists to prove
        // is not happening.
        let backing = LockedValues()
        let source = EnvTokenSource { backing.value(for: $0) }
        let empty = try await source.token()
        XCTAssertEqual(empty, "", "an unset environment yields no credential")

        backing.set("LOAMS_TOKEN", "from-env")
        let afterSet = try await source.token()
        XCTAssertEqual(afterSet, "from-env", "a value set after construction is picked up")
    }

    /// `LOAMS_API_KEY` wins over `LOAMS_TOKEN`, in the order the design names.
    func testEnvironmentPrefersAPIKey() async throws {
        let both: [String: String] = ["LOAMS_API_KEY": "key", "LOAMS_TOKEN": "token"]
        let source = EnvTokenSource { both[$0] }
        let token = try await source.token()
        XCTAssertEqual(token, "key", "LOAMS_API_KEY is read first")
        XCTAssertEqual(EnvTokenSource.names, ["LOAMS_API_KEY", "LOAMS_TOKEN"])
    }

    /// An API key does not expire, so R1's refresh is the no-op the contract
    /// describes.
    func testAPIKeyRefreshIsANoOp() async throws {
        let source = APIKeyTokenSource("loams_key")
        try await source.refresh()
        try await source.refresh()
        let token = try await source.token()
        XCTAssertEqual(token, "loams_key", "a refresh must not disturb a key that does not expire")
    }
}

// MARK: - R7 — streams

final class StreamContractTests: XCTestCase {
    /// A failure the retry class does not cover — notably an `unimplemented`
    /// stream, which is what every `loams.live.v1` RPC answers in the standard
    /// variant — is **reported** rather than spun on.
    func testUnimplementedStreamIsReportedNotRetried() async throws {
        let transport = StubTransport(replies: [
            .failure(
                code: .unimplemented,
                message: "loams.live.v1 is not served in this variant",
                info: ErrorInfoShape(
                    reason: Reason.featureNotInVariant.rawValue,
                    metadata: ["variant": "standard"]
                )
            )
        ])
        let invoker = CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        let binding = try loamsRequireBinding(module: "live", call: "Watch")
        // `Response` is spelled out because the stream's type is only pinned
        // by the assignment's use of `policy`, which leaves it unconstrained.
        var stream: ResumableServerStream<DynamicMessage, DynamicMessage> = try await invoker.serverStream(
            binding: binding,
            request: DynamicMessage(protoTypeName: watchRequestTypeName)
        )
        stream.policy = StreamResume(
            resume: { _, request in request },
            cursorOf: { _ in StreamCursor.none }
        )

        do {
            for try await _ in stream.messages() { }
            XCTFail("an unimplemented stream must be reported")
        } catch let error as FeatureNotInVariantError {
            XCTAssertEqual(error.variant, "standard", "the variant comes from metadata.variant")
        }
        XCTAssertEqual(
            transport.calls.count, 1,
            "an unimplemented stream must not be spun on: \(transport.calls.count) opens"
        )
    }

    /// Without a resume policy a broken stream is reported, not reconnected. This
    /// is the default, and R7 asks for it.
    func testNoResumePolicyMeansNoReopen() async throws {
        let transport = StubTransport(
            replies: [.success([:])],
            streamPlan: [
                [
                    .success(["state_version": .string("10")]),
                    .failure(code: .unavailable, message: "restarting", info: nil),
                ],
                [.success(["state_version": .string("30")])],
            ]
        )
        let invoker = CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        let binding = try loamsRequireBinding(module: "live", call: "Watch")
        // `Response` is spelled out because the stream's type is only pinned
        // by the assignment's use of `policy`, which leaves it unconstrained.
        var stream: ResumableServerStream<DynamicMessage, DynamicMessage> = try await invoker.serverStream(
            binding: binding,
            request: DynamicMessage(protoTypeName: watchRequestTypeName)
        )
        // No `stream.policy` set.
        var versions: [String] = []
        do {
            for try await message in stream.messages() {
                versions.append(message.fields["state_version"]?.stringValue ?? "")
            }
            XCTFail("a broken stream without a resume policy must report")
        } catch {
            XCTAssertEqual(LoamsError.code(of: error), .unavailable)
        }
        XCTAssertEqual(versions, ["10"], "the message before the break is still delivered")
        XCTAssertEqual(transport.calls.count, 1, "and no re-open is attempted")
    }
}

// MARK: - R8 — the reason registry

final class ReasonRegistryTests: XCTestCase {
    /// The registry is the contract. A reason the registry has lost must stop
    /// compiling, which is why this asserts the **count** as well as the set.
    func testReasonRegistryHasAllTwentySix() {
        XCTAssertEqual(
            loamsAllReasons.count, 26,
            "docs/api/reasons.md lists 26 reasons; the SDK's registry must match it exactly"
        )
    }

    /// Every reason has a code, because D611's "the code gives the class" is only
    /// true if every reason is raised under one.
    func testEveryReasonHasACode() {
        for reason in loamsAllReasons {
            XCTAssertNotNil(
                loamsReasonCodes[reason],
                "\(reason.rawValue) has no code; D611's class would be undefined for it"
            )
        }
        XCTAssertEqual(
            loamsReasonCodes.count, loamsAllReasons.count,
            "no reason may map to a code that is not in the registry"
        )
    }

    /// A reason off the registry is `nil`, not a guess.
    func testUnknownReasonIsNotGuessed() {
        XCTAssertNil(loamsReason(from: "not_a_reason"))
        XCTAssertNil(loamsReason(from: ""))
        XCTAssertEqual(loamsReason(from: "not_found"), .notFound)
        XCTAssertTrue(loamsIsKnownReason(.notFound))
        // A reason off the registry has no `Reason` at all, so it cannot be fed
        // to `loamsIsKnownReason(_:)` — the assertion is that the lookup is
        // `nil`, which the two `XCTAssertNil` lines above already make. What is
        // left to check here is that a *known* reason round-trips, so a registry
        // entry that fails to map back cannot pass unnoticed.
        XCTAssertEqual(loamsReason(from: "aborted"), .aborted)
        XCTAssertTrue(loamsIsKnownReason(.aborted))
    }

    /// The two reasons the SDK's own logic branches on, checked by code as well as
    /// by name — a registry edit that moved one to the wrong code would make R1 and
    /// R5's dispatch fire on the wrong failures.
    func testTheTwoBranchedReasonsHaveTheirCodes() {
        XCTAssertEqual(loamsReasonCodes[.featureNotInVariant], .unimplemented)
        XCTAssertEqual(loamsReasonCodes[.tokenExpired], .unauthenticated)
    }
}

// MARK: - R10 / transport

final class TransportContractTests: XCTestCase {
    /// The four content types the recorded corpus carries. An SDK that sent only
    /// one of them would fail three quarters of the fixtures.
    func testContentTypesCoverTheCorpus() {
        XCTAssertEqual(
            ConnectTransport.contentType(protocol: .connect, codec: .json, streaming: false),
            "application/json"
        )
        XCTAssertEqual(
            ConnectTransport.contentType(protocol: .connect, codec: .json, streaming: true),
            "application/connect+json"
        )
        XCTAssertEqual(
            ConnectTransport.contentType(protocol: .grpcWeb, codec: .json, streaming: false),
            "application/grpc-web+json"
        )
        XCTAssertEqual(
            ConnectTransport.contentType(protocol: .connect, codec: .proto, streaming: true),
            "application/connect+proto"
        )
    }

    /// The five-byte envelope: a flags byte, a big-endian length, then the payload.
    func testEnvelopeRoundTrips() {
        let payload = Data("hello".utf8)
        let framed = ConnectTransport.envelope(payload: payload, flags: 0)
        XCTAssertEqual(framed.count, 5 + payload.count)
        XCTAssertEqual(framed[0], 0, "the flags byte comes first")

        let frames = ConnectTransport.parseEnvelopes(framed)
        XCTAssertEqual(frames.count, 1)
        guard case .message(let decoded) = frames[0] else {
            return XCTFail("a 0x00 frame is a message, got \(frames[0])")
        }
        XCTAssertEqual(decoded, payload)
    }

    /// The end-stream frame (0x02) and the gRPC-Web trailers frame (0x80) are both
    /// distinguishable from a message, because the refusal for an absent package
    /// arrives in the end frame rather than as an HTTP status.
    func testEndAndTrailerFramesAreDistinguished() {
        let end = ConnectTransport.envelope(payload: Data("{\"error\":{}}".utf8), flags: 0x02)
        let trailers = ConnectTransport.envelope(payload: Data("grpc-status: 12\r\n".utf8), flags: 0x80)
        let frames = ConnectTransport.parseEnvelopes(end + trailers)
        XCTAssertEqual(frames.count, 2)
        if case .end = frames[0] {} else { XCTFail("0x02 must be an end frame") }
        if case .trailers = frames[1] {} else { XCTFail("0x80 must be a trailers frame") }
    }

    /// A response cut short by a dropped connection has no complete frame, and the
    /// honest report is "the stream stopped", not "the SDK could not parse".
    func testTruncatedEnvelopeIsDroppedNotThrown() {
        let framed = ConnectTransport.envelope(payload: Data(repeating: 0x41, count: 100), flags: 0)
        let truncated = framed.prefix(framed.count - 50)
        XCTAssertTrue(
            ConnectTransport.parseEnvelopes(Data(truncated)).isEmpty,
            "a partial trailing frame yields no frame rather than throwing"
        )
    }

    /// A clean end is the literal `{}`, and it is **not** an error.
    func testCleanEndStreamIsNotAFailure() {
        XCTAssertNil(ConnectTransport.endStreamFailure(Data("{}".utf8)))
        XCTAssertNil(ConnectTransport.endStreamFailure(Data()))
    }

    /// The end frame's `error` becomes a typed failure, because that is where a
    /// server-streamed refusal actually arrives.
    func testEndStreamErrorBecomesATypedFailure() throws {
        let error = ConnectTransport.endStreamFailure(
            Data("""
                {"error":{"code":"unimplemented","message":"not served","details":[]}}
                """.utf8)
        )
        guard let failure = error as? LoamsError else {
            return XCTFail("an end-frame error must be typed, got \(String(describing: error))")
        }
        XCTAssertEqual(failure.code, .unimplemented)
    }

    /// gRPC-Web puts `grpc-status` in the trailers, so a client that only reads
    /// HTTP status codes sees a 200 on a refusal. This is the case that
    /// distinguishes a real Connect implementation from a status-code-only fake.
    func testGrpcWebStatusIsReadFromTrailers() {
        let trailers = ConnectTransport.trailers(
            fromFrame: Data("grpc-status: 12\r\ngrpc-message: not served\r\n".utf8)
        )
        XCTAssertEqual(trailers["grpc-status"], "12")
        XCTAssertEqual(ConnectTransport.code(grpcStatus: "12"), .unimplemented)
        XCTAssertNil(
            ConnectTransport.code(grpcStatus: "0"),
            "grpc-status 0 is OK, which is the absence of a failure rather than one"
        )
    }

    /// The `.proto` codec is **not** implemented, and it says so rather than
    /// silently sending JSON under an `application/proto` content type — which a
    /// server would answer with a parse error, sending the caller looking at a
    /// content-type mismatch instead of at the real cause.
    func testProtoCodecRefusesRatherThanMisencoding() {
        XCTAssertThrowsError(try ConnectTransport.encodeBody([:], codec: .proto)) { error in
            XCTAssertTrue(
                "\(error)".contains("not implemented"),
                "the refusal must name the cause: \(error)"
            )
        }
        XCTAssertNoThrow(try ConnectTransport.encodeBody(["a": .int(1)], codec: .json))
    }

    /// Deterministic JSON: the corpus compares **request bodies byte for byte**, so
    /// an encoder that emitted keys in hash order would fail a test that has
    /// nothing to do with the SDK.
    func testJSONEncodingIsDeterministic() throws {
        let fields: [String: JSONValue] = ["z": .int(1), "a": .int(2), "m": .string("x")]
        let first = try JSONValue.object(fields).encoded()
        for _ in 0..<50 {
            XCTAssertEqual(try JSONValue.object(fields).encoded(), first, "encoding is not deterministic")
        }
    }
}

// MARK: - R8 — the ErrorInfo codec

final class ErrorInfoCodecTests: XCTestCase {
    /// The `ErrorInfo` the runtime reads for `reason`, round-tripping through the
    /// real protobuf encoding.
    func testErrorInfoRoundTrips() throws {
        let info = ErrorInfoShape(
            reason: Reason.featureNotInVariant.rawValue,
            metadata: ["variant": "standard", "package": "loams.live.v1"],
            hint: "this build variant does not carry loams.live.v1"
        )
        let decoded = ConnectTransport.errorInfo(fromProto: loamsEncodeErrorInfo(info))
        XCTAssertEqual(decoded, info, "reason, metadata and hint must all survive")
    }

    /// An `ErrorInfo` with only a `reason`, which is the minimum a server may send.
    func testErrorInfoWithOnlyAReason() throws {
        let decoded = ConnectTransport.errorInfo(
            fromProto: loamsEncodeErrorInfo(ErrorInfoShape(reason: "not_found"))
        )
        XCTAssertEqual(decoded, ErrorInfoShape(reason: "not_found"))
    }

    /// A newer server adds a field. Parsing must still succeed — R8's "surfaced,
    /// not dropped" depends on the decode working at all — and the known fields
    /// must survive.
    func testUnknownFieldIsSkippedAndKnownOnesSurvive() throws {
        var payload = loamsEncodeErrorInfo(ErrorInfoShape(reason: "not_found", hint: "check the id"))
        // Field 9, wire type 2 (length-delimited): a field this SDK has never heard of.
        payload.append(loamsProtoTag(field: 9, wireType: 2))
        payload.append(loamsProtoVarint(3))
        payload.append(contentsOf: Array("new".utf8))

        let decoded = ConnectTransport.errorInfo(fromProto: payload)
        XCTAssertEqual(decoded?.reason, "not_found", "the reason survives an unknown field")
        XCTAssertEqual(decoded?.hint, "check the id")
    }

    /// A malformed payload must not produce a **half-built** value. Returning nil
    /// means "no ErrorInfo", which the caller reports as a Loams failure with no
    /// reason — rather than a failure with a reason that was never on the wire.
    func testMalformedPayloadDecodesToNilNotAPartialValue() {
        // A length prefix that runs past the end of the buffer.
        var truncated = loamsProtoTag(field: 1, wireType: 2)
        truncated.append(loamsProtoVarint(200))
        truncated.append(contentsOf: Array("short".utf8))
        XCTAssertNil(
            ConnectTransport.errorInfo(fromProto: truncated),
            "a truncated field yields no ErrorInfo rather than a partial one"
        )
        // Groups (wire type 3) are not proto3, so their appearance means the
        // payload is not a message this reader understands.
        XCTAssertNil(
            ConnectTransport.errorInfo(fromProto: Data([0x0B, 0x00])),
            "a group header is refused rather than desynchronising the cursor"
        )
    }

    /// The `ErrorInfo` is found **by type**, never by position, so a service that
    /// adds a detail of its own does not move `reason` out from under a caller.
    func testErrorInfoIsFoundByTypeNotPosition() {
        let own = #"{"type":"loams.something.v1.MyOwnDetail","value":"AA=="}"#
        let errorInfo = #"{"type":"loams.errors.v1.ErrorInfo","value":"\#(Data(loamsEncodeErrorInfo(ErrorInfoShape(reason: "not_found"))).base64EncodedString())"}"#
        let details = [
            JSONValue.decoded(fromUTF8: own)!,
            JSONValue.decoded(fromUTF8: errorInfo)!,
        ]
        let found = ConnectTransport.errorInfo(fromDetails: details)
        XCTAssertEqual(found?.reason, "not_found", "a foreign detail in front must not hide the ErrorInfo")
    }

    /// A detail carrying a `type.googleapis.com/` prefix names the same message, so
    /// both spellings match.
    func testFullyQualifiedTypeURLMatches() {
        let encoded = Data(loamsEncodeErrorInfo(ErrorInfoShape(reason: "aborted"))).base64EncodedString()
        let detail = JSONValue.object([
            "type": .string("type.googleapis.com/loams.errors.v1.ErrorInfo"),
            "value": .string(encoded),
        ])
        // `ErrorInfoShape.reason` is the raw string off the wire, so it is
        // compared as one; mapping it to a `Reason` is `loamsReason(from:)`'s job.
        XCTAssertEqual(
            ConnectTransport.errorInfo(fromDetails: [detail])?.reason,
            "aborted",
            "a type.googleapis.com/ prefix names the same message and must match"
        )
    }

    /// An integer must survive as an integer. `page_size` coming back as `1000.0`
    /// would be a silent type change on a field the iterator writes.
    func testJSONIntegersStayIntegers() throws {
        let encoded = try JSONValue.object(["page_size": .int(1000)]).encoded()
        let decoded = try JSONValue.decoded(from: encoded)
        XCTAssertEqual(decoded.objectValue?["page_size"], .int(1000))
        XCTAssertEqual(decoded.objectValue?["page_size"]?.intValue, 1000)
    }
}

// MARK: - The facade

final class FacadeContractTests: XCTestCase {
    /// Every binding in the catalogue resolves, and every module method's binding
    /// exists. A missing one is an `internal` failure at call time, which is a much
    /// worse place to find out than here.
    func testEveryBindingResolves() throws {
        XCTAssertFalse(loamsModules.isEmpty, "the module catalogue is empty")
        for module in loamsModules {
            XCTAssertFalse(module.calls.isEmpty, "\(module.name) has no calls")
            for call in module.calls {
                XCTAssertEqual(
                    loamsBinding(module: module.name, call: call.name)?.rpc, call.rpc,
                    "\(module.name).\(call.name) does not resolve to itself"
                )
                // Both spellings a caller might use must work.
                XCTAssertNotNil(
                    loamsBinding(module: module.name, call: call.protoName),
                    "\(module.name).\(call.protoName) (the proto spelling) must resolve"
                )
            }
        }
    }

    /// `loams.live` and `loams.tables` are two facade names for one package, so the
    /// guard on either must cover the other.
    func testDerivedModulesShareOnePackage() {
        XCTAssertEqual(
            loamsModulesForPackage("loams.live.v1").sorted(), ["live", "tables"],
            "both names for loams.live.v1 must be listed, or a guard on one misses the other"
        )
        let tables = loamsModule("tables")
        XCTAssertEqual(tables?.derived, true, "tables is a derived module and says so")
        XCTAssertEqual(tables?.package, loamsModule("live")?.package)
    }

    /// The retry class is **read from the binding**, not guessed per call site. A
    /// mutation is `.manual` and only `Mutate` carries a key.
    func testRetryClassesAndIdempotencyComeFromTheBindings() throws {
        let getInstance = try loamsRequireBinding(module: "instance", call: "GetInstance")
        XCTAssertEqual(getInstance.retry, .safe, "a read retries on its own")
        XCTAssertEqual(getInstance.idempotency, .noSideEffects)

        let mutate = try loamsRequireBinding(module: "tables", call: "Mutate")
        XCTAssertEqual(mutate.retry, .manual, "a mutation does not retry on its own")
        XCTAssertTrue(mutate.takesIdempotencyKey, "MutateRequest declares idempotency_key")

        let deploy = try loamsRequireBinding(module: "tables", call: "Deploy")
        XCTAssertEqual(deploy.retry, .manual)
        XCTAssertFalse(
            deploy.takesIdempotencyKey,
            "DeployRequest has no idempotency_key; keying it would invent a field the schema lacks"
        )
        XCTAssertFalse(
            deploy.isRetrySafe,
            "an unkeyed mutation is not retry-safe"
        )
    }

    /// A missing binding reports `internal` naming both halves, because it is a bug
    /// in the facade rather than in the caller's code (D611).
    func testMissingBindingIsAnInternalFailure() {
        XCTAssertThrowsError(try loamsRequireBinding(module: "collection", call: "ListCollections")) { error in
            XCTAssertEqual(LoamsError.code(of: error), .internal)
            XCTAssertTrue(
                "\(error)".contains("ListCollections"),
                "the failure names the call that does not resolve: \(error)"
            )
            XCTAssertTrue(
                "\(error)".contains("loams.options.v1"),
                "and names the annotation that is missing, so the fix is findable: \(error)"
            )
        }
    }

    /// `google.protobuf` is the well-known types, and a client never asks an
    /// instance to serve them — including it would make every instance look
    /// incompatible.
    func testProtoPackagesIncludeTheWellKnownTypes() {
        XCTAssertTrue(loamsProtoPackages.contains("google.protobuf"))
        let loamsOnes = loamsProtoPackages.filter { $0.hasPrefix("loams.") }
        XCTAssertEqual(loamsOnes.count, 8, "eight loams.* packages, which is what R9 compares")
    }
}

// MARK: - The client

final class ClientContractTests: XCTestCase {
    /// An empty or schemeless endpoint is a usage error rather than a request to
    /// localhost: a client that quietly talks to the wrong instance is worse than
    /// one that does not start.
    func testEndpointMustBeAbsoluteHTTP() {
        XCTAssertThrowsError(try Loams(Options(endpoint: URL(string: "not a url")!)))
        XCTAssertThrowsError(try Loams(Options(endpoint: URL(fileURLWithPath: "/tmp/x"))))
        XCTAssertNoThrow(try Loams(Options(endpoint: URL(string: "https://acme.loams.dev")!)))
        XCTAssertNoThrow(try Loams(Options(endpoint: URL(string: "http://127.0.0.1:8080")!)))
    }

    /// A retry budget that does not come from asking is the default, not none.
    func testMaxRetriesDefaultsToTheDesignNumber() async throws {
        let client = try Loams(Options(endpoint: URL(string: "https://acme.loams.dev")!))
        let budget = await client.invoker.defaultMaxRetries
        XCTAssertEqual(
            budget, loamsDefaultMaxRetries,
            "an SDK that silently did not retry unless configured would be a sharp edge"
        )

        let optedOut = try Loams(
            Options(endpoint: URL(string: "https://acme.loams.dev")!, noRetries: true)
        )
        let none = await optedOut.invoker.defaultMaxRetries
        XCTAssertEqual(none, 0, "noRetries is the explicit opt-out")

        let explicit = try Loams(
            Options(endpoint: URL(string: "https://acme.loams.dev")!, maxRetries: 7)
        )
        let seven = await explicit.invoker.defaultMaxRetries
        XCTAssertEqual(seven, 7, "an explicit number is honoured")
    }

    /// R4's session is **off by default**, and `nil` rather than an inert store so
    /// "not on" and "on but empty" do not look the same.
    func testSessionConsistencyIsOffByDefault() async throws {
        let off = try Loams(Options(endpoint: URL(string: "https://acme.loams.dev")!))
        let none = await off.session
        XCTAssertNil(none, "the session is off unless asked for")

        let on = try Loams(
            Options(endpoint: URL(string: "https://acme.loams.dev")!, sessionConsistency: true)
        )
        let store = await on.session
        XCTAssertNotNil(store, "sessionConsistency: true turns it on")
        let current = await store?.current()
        XCTAssertNil(current, "and it starts empty, which is a different state from off")
    }
}
/// A dictionary a `@Sendable` closure can read and an `async` test can write.
///
/// The alternative — a captured `var` — is exactly the bug
/// `testEnvironmentIsReadEveryCall` exists to rule out, so the test's own
/// fixture must not commit it.
final class LockedValues: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [String: String] = [:]

    func value(for key: String) -> String? {
        lock.withLock { storage[key] }
    }

    func set(_ key: String, _ value: String) {
        lock.withLock { storage[key] = value }
    }
}
