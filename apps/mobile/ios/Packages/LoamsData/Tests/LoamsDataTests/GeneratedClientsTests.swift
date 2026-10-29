import Connect
import Foundation
import LoamsProto
import XCTest

/// AP3 Task 1: the committed LoamsProto sources hold a connect-swift client for every service
/// in the vendored protos, so a proto change that adds a service cannot ship without its client.
final class GeneratedClientsTests: XCTestCase {
    /// apps/mobile, from Tests/LoamsDataTests/GeneratedClientsTests.swift.
    private var mobileRoot: URL {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { url.deleteLastPathComponent() }
        return url
    }

    /// `loams.approvals.v1` and `ApprovalService` -> `Loams_Approvals_V1_ApprovalServiceClient`,
    /// swift-protobuf's and connect-swift's naming for a package and a service.
    static func clientName(package: String, service: String) -> String {
        let prefix = package.split(separator: ".").map { part in part.prefix(1).uppercased() + part.dropFirst() }.joined(separator: "_")
        return "\(prefix)_\(service)Client"
    }

    func test_generated_clients_exist_for_every_service() throws {
        let protoDir = mobileRoot.appendingPathComponent("proto")
        let generated = mobileRoot.appendingPathComponent("ios/Packages/LoamsProto/Sources/LoamsProto/Generated")
        let enumerator = try XCTUnwrap(FileManager.default.enumerator(atPath: protoDir.path))
        var services = 0
        while let relative = enumerator.nextObject() as? String {
            guard relative.hasSuffix(".proto") else { continue }
            let text = try String(contentsOf: protoDir.appendingPathComponent(relative), encoding: .utf8)
            guard let package = Self.firstCapture(#"(?m)^package\s+([A-Za-z0-9_.]+)\s*;"#, in: text) else { continue }
            let names = Self.allCaptures(#"(?m)^service\s+([A-Za-z0-9_]+)\s*\{"#, in: text)
            guard !names.isEmpty else { continue }
            // FileNaming=PathToUnderscores: loams/approvals/v1/approvals.proto -> loams_approvals_v1_approvals
            let stem = String(relative.dropLast(".proto".count)).replacingOccurrences(of: "/", with: "_")
            let file = generated.appendingPathComponent("\(stem).connect.swift")
            let source = try String(contentsOf: file, encoding: .utf8)
            for name in names {
                services += 1
                let client = Self.clientName(package: package, service: name)
                XCTAssertTrue(source.contains("public final class \(client):"), "\(file.lastPathComponent) has no \(client)")
                XCTAssertTrue(source.contains("public protocol \(client)Interface"), "\(file.lastPathComponent) has no \(client)Interface")
            }
        }
        XCTAssertEqual(services, 5, "instance, devices, approvals, operations and notifications")

        // And the generated types are the ones the app links against.
        let client = ProtocolClient(
            httpClient: URLSessionHTTPClient(),
            config: ProtocolClientConfig(host: "https://loams.invalid", networkProtocol: .connect, codec: ProtoCodec())
        )
        let all: [Any] = [
            Loams_Instance_V1_InstanceServiceClient(client: client),
            Loams_Devices_V1_DeviceServiceClient(client: client),
            Loams_Approvals_V1_ApprovalServiceClient(client: client),
            Loams_Operations_V1_OperationsServiceClient(client: client),
            Loams_Notifications_V1_NotificationServiceClient(client: client),
        ]
        XCTAssertEqual(all.count, services)
    }

    func test_client_names_follow_the_generator() {
        XCTAssertEqual(Self.clientName(package: "loams.approvals.v1", service: "ApprovalService"), "Loams_Approvals_V1_ApprovalServiceClient")
    }

    private static func firstCapture(_ pattern: String, in text: String) -> String? {
        allCaptures(pattern, in: text).first
    }

    private static func allCaptures(_ pattern: String, in text: String) -> [String] {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return [] }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        return regex.matches(in: text, range: range).compactMap { match in
            Range(match.range(at: 1), in: text).map { String(text[$0]) }
        }
    }
}
