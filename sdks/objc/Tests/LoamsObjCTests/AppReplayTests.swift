import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import XCTest
import LoamsObjC

/// The fixture server, started once for the replay.
final class ReplayServer: @unchecked Sendable {
    static let shared = ReplayServer()
    private var process: Process?
    private(set) var endpoint: String?

    func start() throws -> String {
        if let endpoint = endpoint { return endpoint }
        let dir = fixturesDir()
        let script = dir.deletingLastPathComponent().appendingPathComponent("conformance/fixture-server.mjs").path
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        proc.arguments = ["node", script, "--port", "0", "--fixtures", dir.path]
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = Pipe()
        try proc.run()
        process = proc
        var text = ""
        let deadline = Date().addingTimeInterval(15)
        while Date() < deadline {
            text += String(data: pipe.fileHandleForReading.availableData, encoding: .utf8) ?? ""
            if let match = text.range(of: "\"url\":\"[^\"]+\"", options: .regularExpression) {
                let url = text[match].components(separatedBy: "\"")[3]
                endpoint = url
                return url
            }
        }
        throw LoamsValues.invalidArgument("the fixture server printed no URL")
    }

    func stop() {
        process?.terminate()
        process = nil
        endpoint = nil
    }
}

/// The recorded app scenario (`sdks/fixtures/recorded/app/*.json`,
/// `app-scenario.md`), replayed through the public facade against the fixture
/// server (SDKP P0d). Every step goes through `LoamsClient.call` with its
/// recorded body; a step that names a builder case is first checked against that
/// builder; the answer is compared with the recording, its `volatile` fields
/// masked, and a refusal by its code. The fixtures replayed are added to
/// `results/objc.json`, which `check-languages.mjs --check objc --suite app` reads.
final class AppReplayTests: XCTestCase {
    override class func tearDown() {
        ReplayServer.shared.stop()
        super.tearDown()
    }

    // Spelled out: `check-languages.mjs` looks for each name in this file.
    func test_objc_app_collections() throws { try replay("app_collections") }
    func test_objc_app_search() throws { try replay("app_search") }
    func test_objc_app_graph() throws { try replay("app_graph") }
    func test_objc_app_sql() throws { try replay("app_sql") }
    func test_objc_app_streams() throws { try replay("app_streams") }

    func replay(_ name: String) throws {
        let file = fixturesDir().appendingPathComponent("recorded/app/\(name).json")
        guard let data = try? Data(contentsOf: file) else { throw XCTSkip("no recording for \(name)") }
        let fixture = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let volatile = Set((fixture["volatile"] as? [String]) ?? [])
        let config = LoamsClientConfiguration(endpoint: try ReplayServer.shared.start(), tokenSource: nil, maxRetries: 1)
        let client = LoamsClient(configuration: config)
        var failures: [String] = []
        for (index, step) in (fixture["steps"] as! [[String: Any]]).enumerated() {
            let label = "\(name) step \(index) (\(step["call"]!))"
            let body = ((step["request"] as! [String: Any])["body"] as? String) ?? ""
            let request = body.isEmpty ? [:] : try JSONSerialization.jsonObject(with: body.data(using: .utf8)!) as! [String: Any]
            if let builder = step["builder"] as? String, let problem = try checkBuilder(builder, request) {
                failures.append("\(label): \(problem)")
            }
            let expected = step["response"] as! [String: Any]
            let answered = (expected["status"] as? Int) == 200
            let options = LoamsCallOptions()
            options.headers = ["loams-fixture-name": name, "loams-fixture-step": String(index)]
            do {
                let answer = try client.call(step["call"] as! String, request: request, options: options)
                if !answered {
                    failures.append("\(label): answered, but the recording is a refusal")
                    continue
                }
                if let diff = compare(mask(expected["body"]!, volatile), mask(answer, volatile), "$") {
                    failures.append("\(label): \(diff)")
                }
            } catch let error as LoamsError {
                let want = (expected["body"] as? [String: Any])?["code"] as? String
                if answered {
                    failures.append("\(label): refused (\(error.codeString)) where the recording answered")
                } else if error.codeString != want {
                    failures.append("\(label): refused with \(error.codeString), the recording with \(want ?? "?")")
                }
            }
        }
        XCTAssertEqual(failures, [], failures.joined(separator: "\n"))
        if failures.isEmpty { record(name) }
    }

    func checkBuilder(_ name: String, _ request: [String: Any]) throws -> String? {
        let file = fixturesDir().appendingPathComponent("builders/\(name).json")
        guard let data = try? Data(contentsOf: file) else { return "no builder case \(name)" }
        let c = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let built = try BuilderCorpus.build(c)
        var part: Any?
        switch c["builder"] as! String {
        case "schema": part = request["schema"]
        case "filter": part = request["where"] ?? request["filter"]
        default:
            var r = request
            r.removeValue(forKey: "namespace")
            part = r
        }
        guard let p = part else { return "the recording has no \(name) part" }
        return normalized(p).isEqual(normalized(built)) ? nil : "the \(name) builder differs from the recording"
    }

    func mask(_ value: Any, _ volatile: Set<String>) -> Any {
        if let d = value as? [String: Any] {
            var out: [String: Any] = [:]
            for (k, v) in d where !volatile.contains(k) { out[k] = mask(v, volatile) }
            return out
        }
        if let l = value as? [Any] { return l.map { mask($0, volatile) } }
        return value
    }

    /// Nil when equal; numbers compare to float32 precision.
    func compare(_ want: Any, _ got: Any?, _ at: String) -> String? {
        if let w = want as? [String: Any] {
            guard let g = got as? [String: Any] else { return "\(at): not an object" }
            for k in Set(w.keys).union(g.keys) {
                guard let wv = w[k] else { return "\(at).\(k): not recorded" }
                if let p = compare(wv, g[k], "\(at).\(k)") { return p }
            }
            return nil
        }
        if let w = want as? [Any] {
            guard let g = got as? [Any], g.count == w.count else { return "\(at): not \(w.count) items" }
            for (i, item) in w.enumerated() {
                if let p = compare(item, g[i], "\(at)[\(i)]") { return p }
            }
            return nil
        }
        if let w = want as? NSNumber, !(want is Bool), let g = got as? NSNumber {
            let a = w.doubleValue, b = g.doubleValue
            return abs(a - b) <= 1e-6 * max(1, abs(a)) ? nil : "\(at): \(b), recorded \(a)"
        }
        return normalized(want).isEqual(normalized(got ?? NSNull())) ? nil : "\(at): \(String(describing: got)), recorded \(want)"
    }

    func record(_ fixture: String) {
        let file = fixturesDir().appendingPathComponent("results/objc.json")
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        var report = (try? Data(contentsOf: file)).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
            ?? ["language": "objc", "transport": "connect", "ran": [String]()]
        let ran = Set((report["ran"] as? [String]) ?? []).union([fixture])
        report["ran"] = ran.sorted()
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: file)
        }
    }
}
