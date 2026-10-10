import LoamsCore
import LoamsData
import XCTest
@testable import Loams

@MainActor
final class AppModelTests: XCTestCase {
    private func activeModel() async -> (AppModel, DemoBackend) {
        let model = AppModel(secrets: MemoryStore(), keys: TestDecisionKeys())
        let demo = DemoBackend()
        model.activate(backend: demo, record: nil)
        model.approvals = await demo.currentApprovals()
        model.approvalsStatus = .live
        return (model, demo)
    }

    func testDestructiveRequiresTypedTarget() async {
        let (model, _) = await activeModel()
        let drop = model.approvals.first { $0.id == "apr_drop_logs" }!
        let msg = await model.decide(drop, decision: .approve, reason: "", typed: "logs")
        XCTAssertEqual(msg, "Type logs-2026 to confirm.")
    }

    func testApproveSignsAndRemovesTheCard() async {
        let (model, demo) = await activeModel()
        let drop = model.approvals.first { $0.id == "apr_drop_logs" }!
        let msg = await model.decide(drop, decision: .approve, reason: "", typed: "logs-2026")
        XCTAssertTrue(msg.hasPrefix("Approved"), msg)
        XCTAssertFalse(model.approvals.contains { $0.id == "apr_drop_logs" })
        let remaining = await demo.currentApprovals()
        XCTAssertFalse(remaining.contains { $0.id == "apr_drop_logs" })
    }

    func testRejectRequiresReason() async {
        let (model, _) = await activeModel()
        let a = model.approvals.first { $0.id == "apr_agent_reindex" }!
        let msg = await model.decide(a, decision: .reject, reason: "", typed: "")
        XCTAssertEqual(msg, "Say why you are rejecting it.")
    }

    func testOfflineDisablesApprove() async {
        let (model, _) = await activeModel()
        model.approvalsStatus = .reconnecting(retryIn: 2, cause: "lost")
        let a = model.approvals.first { $0.id == "apr_agent_reindex" }!
        let msg = await model.decide(a, decision: .approve, reason: "", typed: "")
        XCTAssertTrue(msg.hasPrefix("Offline"), msg)
    }

    func testBadPairingPayloadSaysSo() async {
        let model = AppModel(secrets: MemoryStore(), keys: TestDecisionKeys())
        await model.pair(payloadText: "https://example.com/not-a-pairing")
        XCTAssertEqual(model.message, "That is not a Loams pairing code.")
    }
}
