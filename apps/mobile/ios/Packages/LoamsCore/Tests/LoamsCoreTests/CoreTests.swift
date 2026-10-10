import Foundation
import XCTest
@testable import LoamsCore

final class PairingTests: XCTestCase {
    func testEverySharedCaseMatches() throws {
        let f = try Fixtures.json("pairing/cases.json")
        let now = Date(timeIntervalSince1970: TimeInterval(Fixtures.int64(f["now"])))
        for case let c as [String: Any] in f["cases"] as! [Any] {
            let name = c["name"] as! String
            let debug = (c["allow_insecure_loopback"] as? Bool) ?? false
            let result = PairingPayloads.parse(c["payload"] as! String, now: now, allowInsecureLoopback: debug)
            let expect = c["expect"] as! String
            switch result {
            case .success(let p):
                XCTAssertEqual(expect, "ok", name)
                let want = c["parsed"] as! [String: Any]
                XCTAssertEqual(p.issuer.absoluteString, want["issuer"] as? String, name)
                XCTAssertEqual(p.instanceID, want["instance_id"] as? String, name)
                XCTAssertEqual(p.jkt, want["jkt"] as? String, name)
                XCTAssertEqual(p.code, want["code"] as? String, name)
                XCTAssertEqual(p.userCode, want["user_code"] as? String, name)
                XCTAssertEqual(Int64(p.exp.timeIntervalSince1970), Fixtures.int64(want["exp"]), name)
                XCTAssertEqual(p.spki, want["spki"] as? [String], name)
            case .failure(let e):
                XCTAssertEqual(e.rawValue, expect, name)
            }
        }
    }
}

final class DecisionTests: XCTestCase {
    private struct ClaimsFile: Decodable {
        struct Case: Decodable {
            struct Claims: Decodable {
                let approval_id: String
                let revision: UInt64
                let decision: String
                let iat: Int64
                let jti: String
            }
            let name: String
            let claims: Claims
            let canonical: String
        }
        let cases: [Case]
    }

    func testDecisionClaimsGoldenBytes() throws {
        // JSONDecoder, not JSONSerialization: the uint64 revision must stay exact.
        let data = try Data(contentsOf: Fixtures.repoRoot.appendingPathComponent("conformance/fixtures/decision/claims.json"))
        for c in try JSONDecoder().decode(ClaimsFile.self, from: data).cases {
            let claims = DecisionClaims(approvalID: c.claims.approval_id, revision: c.claims.revision, decision: Decision(rawValue: c.claims.decision)!, iat: c.claims.iat, jti: c.claims.jti)
            XCTAssertEqual(String(decoding: claims.canonicalJSON(), as: UTF8.self), c.canonical, c.name)
        }
    }

    func testJwsSignatureVerifies() throws {
        let key = P256.Signing.PrivateKey()
        let claims = DecisionClaims(approvalID: "apr_1", revision: 3, decision: .approve, iat: 1_790_899_200, jti: "j")
        let input = Jws.signingInput(payload: claims.canonicalJSON())
        let sig = try key.signature(for: Data(input.utf8))
        XCTAssertEqual(sig.rawRepresentation.count, 64)
        let jws = Jws.compact(signingInput: input, rawSignature: sig.rawRepresentation)
        let parts = jws.split(separator: ".")
        XCTAssertEqual(parts.count, 3)
        let back = try P256.Signing.ECDSASignature(rawRepresentation: Base64URL.decode(String(parts[2]))!)
        XCTAssertTrue(key.publicKey.isValidSignature(back, for: Data(input.utf8)))
    }

    func testRejectRequiresReason() {
        XCTAssertEqual(DecisionRules.check(decision: .reject, pending: true, destructive: false, confirmText: "", typed: "", reason: " ", live: true), .reasonRequired)
    }

    func testDestructiveRequiresTypedTarget() {
        XCTAssertEqual(DecisionRules.check(decision: .approve, pending: true, destructive: true, confirmText: "logs-2026", typed: "logs", reason: "", live: true), .confirmationMismatch)
        XCTAssertNil(DecisionRules.check(decision: .approve, pending: true, destructive: true, confirmText: "logs-2026", typed: "logs-2026", reason: "", live: true))
    }

    func testOfflineDisablesApprove() {
        XCTAssertEqual(DecisionRules.check(decision: .approve, pending: true, destructive: false, confirmText: "", typed: "", reason: "", live: false), .offline)
    }
}

final class PushTests: XCTestCase {
    private func fixture() throws -> [String: Any] { try Fixtures.json("push/sealed.json") }

    func testUnsealMatchesMockFixture() throws {
        let f = try fixture()
        let key = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: Data(base64Encoded: f["recipient_private_key"] as! String)!)
        let plain = try Unsealer.open(Data(base64Encoded: f["sealed"] as! String)!, instanceID: f["instance_id"] as! String, notificationID: f["notification_id"] as! String, privateKey: key)
        XCTAssertEqual(plain, Data(base64Encoded: f["plaintext"] as! String)!)
        XCTAssertEqual(Unsealer.info(instanceID: f["instance_id"] as! String, notificationID: f["notification_id"] as! String), Data(base64Encoded: f["info"] as! String)!)
    }

    func testUnsealRefusesWrongBinding() throws {
        let f = try fixture()
        let key = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: Data(base64Encoded: f["recipient_private_key"] as! String)!)
        XCTAssertThrowsError(try Unsealer.open(Data(base64Encoded: f["sealed"] as! String)!, instanceID: f["instance_id"] as! String, notificationID: "ntf_other", privateKey: key))
    }

    func testSealThenOpen() throws {
        let key = Curve25519.KeyAgreement.PrivateKey()
        let sealed = try Unsealer.seal(Data("hi".utf8), instanceID: "i", notificationID: "n", publicKey: key.publicKey)
        XCTAssertEqual(try Unsealer.open(sealed, instanceID: "i", notificationID: "n", privateKey: key), Data("hi".utf8))
    }

    func testFailureShowsGenericText() throws {
        let f = try fixture()
        let message = f["message"] as! [String: Any]
        let opener = PushOpener(keyFor: { _ in Curve25519.KeyAgreement.PrivateKey() }, decode: { _ in Shown(title: "t", body: "b", notificationID: nil, approvalID: nil) })
        XCTAssertEqual(opener.open(message), .generic)
        XCTAssertEqual(Set(message.keys), PushMessage.fields)
    }
}

final class SmallTests: XCTestCase {
    func testThumbprintsMatchRfcVectors() throws {
        let f = try Fixtures.json("jwk/thumbprints.json")
        for case let c as [String: Any] in f["cases"] as! [Any] {
            let j = c["jwk"] as! [String: String]
            XCTAssertEqual(try Jwk(kty: j["kty"]!, crv: j["crv"]!, x: j["x"]!, y: j["y"]).thumbprint(), c["thumbprint"] as? String)
        }
    }

    func testPkceMatchesRfc7636() throws {
        let f = try Fixtures.json("pkce/rfc7636.json")
        XCTAssertEqual(Pkce.from(verifier: f["code_verifier"] as! String).challenge, f["code_challenge"] as? String)
        XCTAssertEqual(Pkce.generate().verifier.count, 43)
    }

    func testReasons() {
        XCTAssertEqual(Reason.fromWire("pairing_used"), .pairingUsed)
        XCTAssertEqual(Reason.fromWire("new_thing"), .unknown)
        XCTAssertEqual(Reason.fromWire(nil), .unknown)
    }

    /// Same algorithm as scripts/proto-hash.sh: ties the app to conformance/proto-ref.lock.
    func testProtoRefMatchesLock() throws {
        let root = Fixtures.repoRoot
        let lock = try String(contentsOf: root.appendingPathComponent("conformance/proto-ref.lock"), encoding: .utf8)
        let want = lock.split(separator: "\n").first { $0.hasPrefix("tree_sha256=") }.map { String($0.dropFirst("tree_sha256=".count)) }
        let protoDir = root.appendingPathComponent("proto")
        var paths: [String] = []
        let e = FileManager.default.enumerator(atPath: protoDir.path)!
        while let p = e.nextObject() as? String {
            if p.hasSuffix(".proto") { paths.append("proto/" + p) }
        }
        let lines = paths.sorted { $0.utf8.lexicographicallyPrecedes($1.utf8) }.map { p -> String in
            let data = try! Data(contentsOf: root.appendingPathComponent(p))
            return "\(data.sha256.hex)  \(p)\n"
        }.joined()
        XCTAssertEqual(Data(lines.utf8).sha256.hex, want)
    }
}

final class WatchTests: XCTestCase {
    struct Item: Sendable, Equatable { let id: String; let v: Int }
    struct Dropped: Error {}

    func testAppliesSnapshotThenChanges() {
        var s = WatchState<Item>(id: { $0.id })
        let t = Date()
        s.apply(.snapshot([Item(id: "a", v: 1), Item(id: "b", v: 1)], cursor: "1", reset: false), at: t)
        s.apply(.upsert(Item(id: "b", v: 2), cursor: "2"), at: t)
        s.apply(.upsert(Item(id: "c", v: 1), cursor: "3"), at: t)
        s.apply(.remove(id: "a", cursor: "4"), at: t)
        s.apply(.heartbeat(cursor: "4"), at: t)
        XCTAssertEqual(s.items, [Item(id: "b", v: 2), Item(id: "c", v: 1)])
        XCTAssertEqual(s.cursor, "4")
    }

    func testDeadAfterThreeMissedHeartbeats() {
        var s = WatchState<Item>(id: { $0.id })
        let t = Date(timeIntervalSince1970: 1000)
        s.apply(.heartbeat(cursor: "1"), at: t)
        XCTAssertFalse(s.isDead(at: t.addingTimeInterval(45)))
        XCTAssertTrue(s.isDead(at: t.addingTimeInterval(45.5)))
    }

    func testBackoffCapsAt10s() {
        var b = Backoff()
        let delays = (0..<12).map { _ in b.next() }
        XCTAssertTrue(delays[0] <= 0.5)
        XCTAssertTrue(delays.allSatisfy { $0 <= 10 })
        XCTAssertTrue(delays.last! >= 5)
    }

    func testResumesFromCursorAfterADrop() async throws {
        actor Calls { var cursors: [String?] = []; func add(_ c: String?) -> Int { cursors.append(c); return cursors.count } }
        let calls = Calls()
        let watch = ResumingWatch<Item>(
            state: WatchState(id: { $0.id }),
            backoff: Backoff(initial: 0.01, maximum: 0.02),
            open: { cursor in
                AsyncThrowingStream { continuation in
                    Task {
                        let n = await calls.add(cursor)
                        if n == 1 {
                            continuation.yield(.snapshot([Item(id: "a", v: 1)], cursor: "5", reset: false))
                            continuation.finish(throwing: Dropped())
                        } else {
                            continuation.yield(.upsert(Item(id: "b", v: 1), cursor: "6"))
                        }
                    }
                }
            },
            observer: { _, _ in }
        )
        let task = Task { await watch.run() }
        for _ in 0..<200 {
            if await watch.state.items.count == 2 { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        task.cancel()
        let items = await watch.state.items
        XCTAssertEqual(items, [Item(id: "a", v: 1), Item(id: "b", v: 1)])
        let cursors = await calls.cursors
        XCTAssertEqual(Array(cursors.prefix(2)), [nil, "5"])
    }
}
