// `loams.live.admin` (LV1 Task 10): `Deploy` moved from `LiveService` to
// `LiveAdminService`, and the engine no longer serves `LiveService/Deploy`
// (LV1 Task 12). These pin that the Swift facade sends every admin call to the
// admin service, with the retry classes and paging the TypeScript and Rust
// facades give it.

import Foundation
import XCTest
@testable import Loams

final class LiveAdminTests: XCTestCase {
    private let admin = "loams.live.v1.LiveAdminService"

    /// The catalogue has the TypeScript and Rust facades' eleven calls, and no
    /// binding anywhere still names `LiveService/Deploy`.
    func testLiveAdminCatalogueMatchesTheOtherFacades() throws {
        let module = try XCTUnwrap(loamsModule("live.admin"))
        XCTAssertEqual(module.service, admin)
        XCTAssertEqual(module.package, "loams.live.v1")
        XCTAssertTrue(module.unstable)
        XCTAssertFalse(module.derived)
        XCTAssertEqual(
            module.calls.map(\.name),
            [
                "ActivateDeployment", "CreateApp", "DeleteApp", "Deploy", "GetApp", "GetDeployment",
                "GetIndexStatus", "ListApps", "ListDeployments", "TailLogs", "UpdateApp",
            ]
        )
        for call in module.calls {
            XCTAssertEqual(call.rpc, "\(admin)/\(call.method)")
            XCTAssertFalse(call.takesIdempotencyKey, "no admin request declares idempotency_key")
        }
        let safe = module.calls.filter { $0.retry == .safe }.map(\.name)
        XCTAssertEqual(
            safe,
            ["GetApp", "GetDeployment", "GetIndexStatus", "ListApps", "ListDeployments"],
            "retry_safe in admin.proto"
        )
        XCTAssertEqual(
            try loamsRequireBinding(module: "live.admin", call: "ListApps").pagination?.itemsField, "apps"
        )
        XCTAssertEqual(try loamsRequireBinding(module: "live.admin", call: "tailLogs").streaming, .server)

        XCTAssertNil(loamsBinding(module: "tables", call: "Deploy"), "Deploy is live.admin's now")
        let stale = loamsModules.flatMap(\.calls).filter { $0.rpc == "loams.live.v1.LiveService/Deploy" }
        XCTAssertTrue(stale.isEmpty, "the engine does not serve LiveService/Deploy: \(stale.map(\.module))")
    }

    /// `client.live.admin.deploy` sends to `LiveAdminService/Deploy`, unkeyed and
    /// not retried; the deprecated `tables.deploy` forwards to the same RPC.
    func testDeployGoesToTheAdminService() async throws {
        let transport = StubTransport(replies: [
            .failure(code: .unavailable, message: "the node is restarting", info: nil),
        ])
        let invoker = CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        let live = LiveModule(invoker: invoker)
        let request = DynamicMessage(protoTypeName: deployRequestTypeName, fields: ["message": .string("v2")])

        do {
            _ = try await live.admin.deploy(request)
            XCTFail("the stub refuses every call")
        } catch {
            XCTAssertEqual(LoamsError.code(of: error), .unavailable)
        }
        XCTAssertEqual(transport.calls.map(\.rpc), ["\(admin)/Deploy"], "an unkeyed mutation is sent once")
        XCTAssertEqual(transport.calls.first?.body, ["message": .string("v2")])

        let forwarded = StubTransport(replies: [.success([:])])
        let tables = TablesModule(invoker: CallInvoker(transport: forwarded, auth: nil, maxRetries: 0))
        _ = try await deprecatedTablesDeploy(tables, request)
        XCTAssertEqual(forwarded.calls.map(\.rpc), ["\(admin)/Deploy"])
    }

    /// The reads are retry-safe, so a transient failure is retried on its own.
    func testAdminReadsRetry() async throws {
        let transport = StubTransport(replies: [
            .failure(code: .unavailable, message: "the node is restarting", info: nil),
            .success(["deployment": .object(["id": .string("01J")])]),
        ])
        let admin = LiveAdminModule(
            invoker: CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        )
        let response = try await admin.getDeployment(
            DynamicMessage(protoTypeName: getDeploymentRequestTypeName, fields: ["id": .string("01J")])
        )
        XCTAssertEqual(response.loamsFields()["deployment"]?.objectValue?["id"], .string("01J"))
        XCTAssertEqual(transport.calls.count, 2, "one unavailable then one success is two attempts")
        XCTAssertEqual(Set(transport.calls.map(\.rpc)), ["\(self.admin)/GetDeployment"])
    }

    /// `listAllDeployments` follows the tokens a real server sends, which are
    /// proto3 JSON (`nextPageToken`), to the end.
    func testListAllDeploymentsFollowsTheWireToken() async throws {
        let transport = StubTransport(replies: [
            .success([
                "deployments": .array([.object(["id": .string("a")]), .object(["id": .string("b")])]),
                "nextPageToken": .string("p2"),
            ]),
            .success(["deployments": .array([.object(["id": .string("c")])])]),
        ])
        let admin = LiveAdminModule(invoker: CallInvoker(transport: transport, auth: nil, maxRetries: 0))

        var ids: [String] = []
        for try await item in admin.listAllDeployments(
            DynamicMessage(protoTypeName: listDeploymentsRequestTypeName, fields: ["page_size": .int(2)])
        ) {
            ids.append(item.objectValue?["id"]?.stringValue ?? "")
        }
        XCTAssertEqual(ids, ["a", "b", "c"])
        XCTAssertEqual(transport.calls.map(\.rpc), Array(repeating: "\(self.admin)/ListDeployments", count: 2))
        XCTAssertNil(transport.calls[0].body["page_token"])
        XCTAssertEqual(transport.calls[1].body["page_token"], .string("p2"))
        XCTAssertEqual(transport.calls[1].body["page_size"], .int(2), "the caller's request is kept")
    }
}

extension LiveAdminTests {
    /// The five app-directory calls (LV1 Task 13) go to `LiveAdminService`; the
    /// mutations are sent once, and `liveApp` puts the app on the wire as the
    /// `loams-live-app` header.
    func testAppCallsGoToTheAdminServiceWithTheAppHeader() async throws {
        let transport = StubTransport(replies: [.success([:])])
        let admin = LiveAdminModule(invoker: CallInvoker(transport: transport, auth: nil, maxRetries: 0))
        let app = CallOptions.liveApp("blog")
        let request = { (type: String) in DynamicMessage(protoTypeName: type, fields: ["name": .string("blog")]) }

        _ = try await admin.createApp(request(createAppRequestTypeName))
        _ = try await admin.listApps(request(listAppsRequestTypeName))
        _ = try await admin.getApp(request(getAppRequestTypeName), options: app)
        _ = try await admin.updateApp(request(updateAppRequestTypeName), options: app)
        _ = try await admin.deleteApp(request(deleteAppRequestTypeName), options: app)

        XCTAssertEqual(
            transport.calls.map(\.rpc),
            ["CreateApp", "ListApps", "GetApp", "UpdateApp", "DeleteApp"].map { "\(self.admin)/\($0)" }
        )
        XCTAssertEqual(loamsLiveAppHeader, "loams-live-app")
        // `connect-timeout-ms` (R12) rides on every unary call; the app header is what is pinned here.
        let appHeaders = transport.calls.map { $0.headers.filter { $0.key != "connect-timeout-ms" } }
        XCTAssertEqual(appHeaders[0], [:], "no app option, no header: the server's own app")
        XCTAssertEqual(appHeaders[2], ["loams-live-app": "blog"])
        XCTAssertEqual(appHeaders[4], ["loams-live-app": "blog"])
    }

    /// `live_app_exists` and `live_app_mismatch` are in the reason registry, and
    /// a failed create is not retried.
    func testAppReasonsAndNoRetryOnCreate() async throws {
        XCTAssertNotNil(loamsReason(from: "live_app_disabled"))
        XCTAssertNotNil(loamsReason(from: "live_app_exists"))
        XCTAssertNotNil(loamsReason(from: "live_app_mismatch"))
        let transport = StubTransport(replies: [
            .failure(code: .unavailable, message: "restarting", info: nil),
        ])
        let admin = LiveAdminModule(
            invoker: CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        )
        do {
            _ = try await admin.createApp(DynamicMessage(protoTypeName: createAppRequestTypeName, fields: [:]))
            XCTFail("the stub refuses every call")
        } catch {
            XCTAssertEqual(LoamsError.code(of: error), .unavailable)
        }
        XCTAssertEqual(transport.calls.count, 1)
    }
}

extension LiveAdminTests {
    /// `client.live.authenticate` (LV1 Task 17) sends `LiveService/Authenticate`
    /// with the client's bearer, is never retried, and returns the identity
    /// version, so a caller tells an extended session (the version it holds) from
    /// a changed identity (a bumped one).
    func testAuthenticateSurfacesExtendedAndChangedIdentity() async throws {
        let call = try loamsRequireBinding(module: "live", call: "authenticate")
        XCTAssertEqual(call.rpc, "loams.live.v1.LiveService/Authenticate")
        XCTAssertEqual(call.retry, .manual)
        XCTAssertEqual(call.streaming, .unary)
        XCTAssertFalse(call.takesIdempotencyKey)

        let transport = StubTransport(replies: [
            .success(["identity": .string("4")]),
            .success(["identity": .string("5")]),
        ])
        let live = LiveModule(
            invoker: CallInvoker(transport: transport, auth: StaticTokenSource("tok-bob"), maxRetries: 0)
        )
        let request = DynamicMessage(
            protoTypeName: authenticateRequestTypeName, fields: ["session_id": .string("s1")]
        )
        let extended = try await live.authenticate(request)
        let changed = try await live.authenticate(request)

        XCTAssertEqual(extended.loamsFields()["identity"], .string("4"), "same caller: the version is unchanged")
        XCTAssertEqual(changed.loamsFields()["identity"], .string("5"), "another caller bumps it")
        XCTAssertEqual(transport.calls.map(\.rpc), Array(repeating: "loams.live.v1.LiveService/Authenticate", count: 2))
        XCTAssertEqual(transport.calls.map(\.bearer), ["tok-bob", "tok-bob"])
        XCTAssertEqual(transport.calls[0].body, ["session_id": .string("s1")])
    }

    /// A refused Authenticate is not retried, and the reasons a Live session
    /// closes with are in the registry.
    func testAuthenticateIsNotRetriedAndSessionReasonsAreKnown() async throws {
        let transport = StubTransport(replies: [.failure(code: .unavailable, message: "restarting", info: nil)])
        let live = LiveModule(
            invoker: CallInvoker(transport: transport, auth: nil, maxRetries: loamsDefaultMaxRetries)
        )
        do {
            _ = try await live.authenticate(DynamicMessage(protoTypeName: authenticateRequestTypeName, fields: [:]))
            XCTFail("the stub refuses every call")
        } catch {
            XCTAssertEqual(LoamsError.code(of: error), .unavailable)
        }
        XCTAssertEqual(transport.calls.count, 1)
        XCTAssertEqual(loamsReason(from: "session_rotate"), .sessionRotate)
        XCTAssertEqual(loamsReason(from: "token_expired"), .tokenExpired)
        XCTAssertEqual(loamsReason(from: "token_revoked"), .tokenRevoked)
    }
}

/// Calls the deprecated method from one place, so the deprecation warning the
/// test exists to cover is not repeated.
@available(*, deprecated)
private func deprecatedTablesDeploy(_ tables: TablesModule, _ request: DynamicMessage) async throws -> DynamicMessage {
    try await tables.deploy(request)
}
