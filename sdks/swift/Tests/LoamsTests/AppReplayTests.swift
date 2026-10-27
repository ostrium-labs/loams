// The recorded app scenario, replayed through the public facade against the
// fixture server (SDKP P0d). `check-languages.mjs --check swift --suite app`
// reads the fixtures these record as run.

import Foundation
import XCTest

final class AppReplayTests: XCTestCase {
    private func replay(_ name: String) async throws {
        guard AppReplay.available(name) else { throw XCTSkip("no recording for \(name)") }
        let server = try FixtureServer.start()
        defer { server.stop() }
        let failures = try await AppReplay.replay(server.endpoint, name)
        XCTAssertTrue(failures.isEmpty, failures.joined(separator: "\n"))
        if failures.isEmpty { try AppReplay.record([name]) }
    }

    // Spelled out: `check-languages.mjs` looks for each name in the suite's sources.
    func test_swift_app_collections() async throws { try await replay("app_collections") }
    func test_swift_app_search() async throws { try await replay("app_search") }
    func test_swift_app_graph() async throws { try await replay("app_graph") }
    func test_swift_app_sql() async throws { try await replay("app_sql") }
    func test_swift_app_streams() async throws { try await replay("app_streams") }
}
