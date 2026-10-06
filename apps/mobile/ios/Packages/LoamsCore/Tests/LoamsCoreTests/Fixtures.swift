import Foundation

/// The repository-root fixtures shared with Android and the mock.
enum Fixtures {
    static let repoRoot: URL = {
        // ios/Packages/LoamsCore/Tests/LoamsCoreTests/Fixtures.swift -> repository root
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { url.deleteLastPathComponent() }
        return url
    }()

    /// JSON numbers come back as Int, Int64 or NSNumber depending on the platform.
    static func int64(_ any: Any?) -> Int64 {
        switch any {
        case let v as Int: return Int64(v)
        case let v as Int64: return v
        case let v as Double: return Int64(v)
        case let v as NSNumber: return v.int64Value
        default: return -1
        }
    }

    static func json(_ path: String) throws -> [String: Any] {
        let data = try Data(contentsOf: repoRoot.appendingPathComponent("conformance/fixtures/\(path)"))
        return try JSONSerialization.jsonObject(with: data) as! [String: Any]
    }
}
