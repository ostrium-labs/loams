import CryptoKit
import Foundation
import LoamsCore
import LoamsProto
import XCTest
@testable import LoamsData

final class DataTests: XCTestCase {
    private var repoRoot: URL {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { url.deleteLastPathComponent() }
        return url
    }

    func testTokenResponseParsesAndRedacts() throws {
        let ok = try TokenEndpoint.parse(status: 200, body: Data(#"{"access_token":"a","token_type":"DPoP","expires_in":3600,"refresh_token":"r","device_id":"dev_1"}"#.utf8))
        guard case .ok(let t) = ok else { return XCTFail("\(ok)") }
        XCTAssertEqual(t.deviceID, "dev_1")
        XCTAssertFalse(t.description.contains("a,") || t.description.contains("\"r\""))
        let refused = try TokenEndpoint.parse(status: 400, body: Data(#"{"error":"invalid_grant","error_description":"used","loams_reason":"pairing_used"}"#.utf8))
        XCTAssertEqual(refused, .refused(error: "invalid_grant", reason: .pairingUsed, description: "used"))
    }

    func testFormEncodingEscapesReservedCharacters() {
        let body = String(decoding: TokenEndpoint.formEncode([("decision_jwk", #"{"kty":"EC"}"#), ("grant_type", Loams.pairingGrant)]), as: UTF8.self)
        XCTAssertEqual(body, "decision_jwk=%7B%22kty%22%3A%22EC%22%7D&grant_type=urn%3Aloams%3Aparams%3Aoauth%3Agrant-type%3Apairing")
    }

    /// The SPKI header + raw key must equal CryptoKit's DER SubjectPublicKeyInfo.
    func testSpkiPinMatchesDerRepresentation() throws {
        let key = P256.Signing.PrivateKey()
        let attrs: [CFString: Any] = [kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom, kSecAttrKeyClass: kSecAttrKeyClassPublic, kSecAttrKeySizeInBits: 256]
        let secKey = try XCTUnwrap(SecKeyCreateWithData(key.publicKey.x963Representation as CFData, attrs as CFDictionary, nil))
        let want = Data(SHA256.hash(data: key.publicKey.derRepresentation)).base64EncodedString()
        XCTAssertEqual(PinValidator.spkiPin(of: secKey), want)
    }

    func testSealedFixtureDecodesWithReviewCategory() throws {
        let data = try Data(contentsOf: repoRoot.appendingPathComponent("conformance/fixtures/push/sealed.json"))
        let f = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let key = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: Data(base64Encoded: f["recipient_private_key"] as! String)!)
        let opener = PushOpener(keyFor: { _ in key }, decode: NotificationDecoding.shown)
        let shown = opener.open(f["message"] as! [String: Any])
        let expect = f["expect"] as! [String: String]
        XCTAssertEqual(shown.title, expect["title"])
        XCTAssertEqual(shown.approvalID, expect["approval_id"])
        XCTAssertEqual(NotificationDecoding.category(Data(base64Encoded: f["plaintext"] as! String)!), .approval)
    }

    func testWatchMappingCoversEveryEvent() {
        var m = Loams_Approvals_V1_WatchApprovalsResponse()
        m.cursor = "7"
        m.event = .remove("apr_1")
        guard case .remove(let id, let c)? = WatchMapping.approvals(m) else { return XCTFail() }
        XCTAssertEqual(id, "apr_1")
        XCTAssertEqual(c, "7")
        m.event = .heartbeat(Loams_Approvals_V1_Heartbeat())
        guard case .heartbeat? = WatchMapping.approvals(m) else { return XCTFail() }
        m.event = nil
        XCTAssertNil(WatchMapping.approvals(m))
    }

    func testDemoDecisionRemovesTheCard() async throws {
        let demo = DemoBackend()
        let drop = await demo.currentApprovals().first { $0.id == "apr_drop_logs" }!
        let outcome = await demo.decide(drop, decision: .approve, reason: "", proof: "x.y.z", idempotencyKey: "k")
        guard case .decided(let a) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertEqual(a.state, .approved)
        let left = await demo.currentApprovals()
        XCTAssertFalse(left.contains { $0.id == "apr_drop_logs" })
    }

    func testSessionRefreshTokenStaysInTheSecretStore() throws {
        let secrets = MemoryStore()
        let store = SessionStore(secrets: secrets)
        try store.save(SessionRecord(issuer: "http://localhost:8084", instanceID: "i", jkt: "j", spki: nil, deviceID: "d", method: "qr"), refreshToken: "secret-refresh")
        XCTAssertEqual(store.refreshToken(), "secret-refresh")
        let defaults = UserDefaults.standard.data(forKey: "loams.session").map { String(decoding: $0, as: UTF8.self) } ?? ""
        XCTAssertFalse(defaults.contains("secret-refresh"))
        store.clear()
        XCTAssertNil(store.refreshToken())
    }
}
