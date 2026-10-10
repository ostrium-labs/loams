// The conformance fixture server (design §44 §10.4, SDK1 Task 4).
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev`, and every
// SDK's suite replays it so thirteen clients can be compared against the same
// bytes. This file is how a Swift test gets one.
//
// Two ways to get an endpoint, in the order they are tried:
//
//  1. `LOAMS_TEST_ENDPOINT` — a live `loams dev`. This short-circuits everything
//     else, which is how `sdks/conformance/run.sh` runs the same suite in CI
//     against a recording and on a developer machine against the real thing.
//  2. `node sdks/conformance/fixture-server.mjs` — the shared server. Preferred,
//     because then the Swift suite really does run against the same server the
//     other twelve do.
//
// # There is no third way, and that is a deliberate difference from the Go SDK
//
// The Go suite's third fallback is an in-process replay of `sdks/fixtures/recorded`
// over `net/http/httptest`, because Go's standard library has an HTTP server and a
// Go module's `go test` should not need a JavaScript runtime.
//
// **Swift has no equivalent in its standard library.** An in-process HTTP server
// would mean hand-writing one over BSD sockets, or taking a dependency on
// SwiftNIO — and a hand-written socket server that is never run because the
// machine has no toolchain is exactly the kind of green-but-unverified thing this
// repository has been burned by. So there is no in-process replay.
//
// What replaces it is that `loamsUnavailableFixtureServer` **fails** rather than
// skipping. A suite that could not reach a fixture server has not run the corpus,
// and reporting that as a pass is precisely the failure mode R7's own doc comment
// warns about for streams. The remedy is one of the two things above, and the
// error message names both.
//
// What all paths agree on is the corpus: ``readCorpus()`` reads
// `sdks/fixtures/index.json` and the test fails if a case is missing, so a suite
// cannot quietly stop covering something.

import Foundation

#if canImport(FoundationNetworking)
// `URLSession` lives in FoundationNetworking on Linux and Foundation on Apple
// platforms. The plan's Task 4 row lists Linux as a target, so every file that
// names `URLSession` or `URLSessionConfiguration` needs this, not just the one
// that makes the request.
import FoundationNetworking
#endif
import XCTest

// `newTestClient` names `Loams`, the SDK's client type. Without this the file
// builds for everything else and fails only on that one function, which reads
// like a missing type rather than a missing import.
import Loams

/// A running fixture endpoint and how to stop it.
struct FixtureServer {
    /// The base URL the client is built with.
    let endpoint: URL
    /// Whether this is a real `loams dev` rather than a replay.
    let live: Bool
    /// Stops the server. A no-op for a live endpoint, which the caller owns.
    let stop: @Sendable () -> Void

    static func start() throws -> FixtureServer {
        let trimmed = (ProcessInfo.processInfo.environment["LOAMS_TEST_ENDPOINT"] ?? "")
            .trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
        if !trimmed.isEmpty, let url = URL(string: trimmed) {
            return FixtureServer(endpoint: url, live: true, stop: {})
        }
        return try startNodeFixtureServer()
    }

    /// Runs the shared fixture server and reads the URL it prints on stdout.
    private static func startNodeFixtureServer() throws -> FixtureServer {
        let node = URL(fileURLWithPath: "#filePath")
            .deletingLastPathComponent()   // Tests
            .deletingLastPathComponent()   // swift
            .appendingPathComponent("conformance/fixture-server.mjs")
        let fixtures = URL(fileURLWithPath: "#filePath")
            .deletingLastPathComponent()   // Tests
            .deletingLastPathComponent()   // swift
            .appendingPathComponent("fixtures")

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["node", node.path, "--fixtures", fixtures.path, "--port", "0"]

        let pipe = Pipe()
        process.standardOutput = pipe
        // stderr goes to the test's own, so a fixture server that explains itself
        // is visible in the failure rather than swallowed.
        process.standardError = FileHandle.standardError

        do {
            try process.run()
        } catch {
            throw loamsNoFixtureServer(lastError: "\(error)")
        }

        // The server prints one JSON line, `{"url": …}`, once it is listening.
        let deadline = Date().addingTimeInterval(30)
        var buffer = Data()
        while Date() < deadline {
            let chunk = pipe.fileHandleForReading.availableData
            if chunk.isEmpty {
                // No more output and the process has not exited: nothing is
                // listening, so stop waiting rather than burning the full 30s.
                break
            }
            buffer.append(chunk)
            // `buffer` is `[UInt8]`, so it decodes directly: passing the
            // optional from an `if let` that was never written made this a
            // `String?` fed to a `Collection` parameter.
            for line in String(decoding: buffer, as: UTF8.self)
                .split(separator: "\n") {
                guard let data = line.data(using: .utf8),
                      let parsed = try? JSONDecoder().decode(FixtureServerBanner.self, from: data),
                      !parsed.url.isEmpty,
                      let url = URL(string: parsed.url)
                else { continue }
                return FixtureServer(endpoint: url, live: false) {
                    // SIGINT rather than `terminate()`, which is SIGKILL: the
                    // server writes nothing on the way out either way, and a
                    // signal it can handle is the one `fixture-server.mjs`
                    // documents.
                    if process.isRunning {
                        kill(process.processIdentifier, SIGINT)
                    }
                }
            }
            if !process.isRunning { break }
            Thread.sleep(forTimeInterval: 0.05)
        }
        if process.isRunning { kill(process.processIdentifier, SIGINT) }
        throw loamsNoFixtureServer(lastError: "the server printed no url within 30s")
    }

    /// The failure a suite reports when it cannot reach a fixture server.
    ///
    /// Deliberately **not** an `XCTSkip`: a skip reads as "this language does not
    /// cover that fixture", which is a claim about the SDK. It is a claim about
    /// the machine, and the honest report of a machine problem is a failure with
    /// both remedies named.
    static func loamsNoFixtureServer(lastError: String) -> NSError {
        NSError(
            domain: "loams.conformance",
            code: 1,
            userInfo: [
                NSLocalizedDescriptionKey: """
                    loams: no conformance fixture server could be reached, so the corpus did NOT run.
                      Last error: \(lastError)

                    Set LOAMS_TEST_ENDPOINT to a `loams dev` that is listening, or put `node` on PATH so \
                    this suite can start sdks/conformance/fixture-server.mjs itself.

                    This is reported as a failure rather than a skip on purpose: a suite that could not \
                    reach the corpus has not passed it.
                    """
            ]
        )
    }
}

/// The line the fixture server prints once it is listening.
private struct FixtureServerBanner: Decodable {
    let url: String
}

/// One entry of `sdks/fixtures/index.json`.
struct FixtureCase: Decodable {
    let name: String
    let about: String
    let path: String
    let contentType: String
    let status: Int
}

/// The corpus index.
struct FixtureCorpus: Decodable {
    let about: String
    let cases: [FixtureCase]
}

/// Reads `sdks/fixtures/index.json`, failing the test if it cannot be read: a
/// suite that cannot see the corpus is not a suite.
func readCorpus() throws -> FixtureCorpus {
    let index = URL(fileURLWithPath: "#filePath")
        .deletingLastPathComponent()   // Tests
        .deletingLastPathComponent()   // swift
        .appendingPathComponent("fixtures/index.json")
    let data = try Data(contentsOf: index)
    return try JSONDecoder().decode(FixtureCorpus.self, from: data)
}

/// Builds an unauthenticated client against a fixture endpoint.
///
/// Unauthenticated because `GetInstance` needs no credential anyway, and because
/// it keeps a bearer out of the test suite entirely.
/// - Parameters:
///   - protocol_: the wire protocol to speak. The corpus records `Watch` only as
///     `application/connect+proto`, so a client that cannot be configured for it
///     cannot replay that fixture at all.
///   - codec: the body encoding. `Watch` is recorded as proto; everything else in
///     the corpus is recorded in all four encodings.
func newTestClient(
    _ endpoint: URL,
    protocol_: WireProtocol = .connect,
    codec: Codec = .json
) throws -> Loams {
    let configuration = URLSessionConfiguration.ephemeral
    // A fixture server on loopback answers in milliseconds; twenty seconds is a
    // hang-detector, not a budget, and a suite that waits twenty seconds per
    // assertion teaches nothing.
    configuration.timeoutIntervalForRequest = 20
    return try Loams(
        Options(
            endpoint: endpoint,
            transport: TransportConfig(endpoint: endpoint, protocol_: protocol_, codec: codec),
            session: URLSession(configuration: configuration)
        )
    )
}