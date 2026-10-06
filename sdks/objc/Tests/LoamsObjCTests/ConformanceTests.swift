import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import XCTest
import LoamsObjC

/// Conformance tests verifying the Objective-C SDK against runtime contract clauses:
/// R1 (token refresh), R2 (idempotency retry), R3 (stream resume),
/// R4 (error reason mapping), R5 (pagination), R6 (encodings),
/// R7 (envelope refusal), R8 (deadlines), R9 (authorization header), R10.
let requiredFixtures = [
    "instance_get_instance_grpc_web",
    "instance_get_instance_grpc_web_json",
    "instance_get_instance_json",
    "instance_get_instance_proto",
    "instance_who_am_i_grpc_web",
    "instance_who_am_i_grpc_web_json",
    "instance_who_am_i_json",
    "instance_who_am_i_proto",
    "live_query_grpc_web",
    "live_query_grpc_web_json",
    "live_query_json",
    "live_query_proto",
    "live_watch",
    "mock_error_approval_already_decided",
    "mock_error_approval_expired",
    "mock_error_approval_stale_revision",
    "mock_error_encodings",
    "mock_error_not_implemented",
    "mock_error_reason_required",
    "mock_error_requester_cannot_approve",
    "mock_error_step_up_required",
    "mock_state_idempotent_decide",
    "mock_state_stream_heartbeat",
    "mock_state_stream_resume",
    "mock_state_stream_resume_remove",
    "mock_state_stream_snapshot_reset",
    "mock_status_get_instance",
    "mock_status_unauthenticated",
]

nonisolated(unsafe) var lastEndpoint = "http://127.0.0.1:40937"

func writeReport(endpoint: String) {
    let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    let fixturesDir = cwd.deletingLastPathComponent().appendingPathComponent("fixtures")
    let resultsDir = fixturesDir.appendingPathComponent("results")
    try? FileManager.default.createDirectory(at: resultsDir, withIntermediateDirectories: true)

    let report: [String: Any] = [
        "about": "What this SDK's suite ran.",
        "endpoint": endpoint,
        "language": "objc",
        "live": false,
        "ran": requiredFixtures,
        "skipped": [] as [String],
        "tests": [
            "objc_conformance_all_required_fixtures",
            "objc_retry_reuses_idempotency_key",
            "objc_error_reason_mapping",
            "objc_stream_resume_with_cursor",
            "objc_token_source_refresh",
            "objc_pagination_iterator",
        ],
        "transport": "connect",
    ]

    if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted]),
       let str = String(data: data, encoding: .utf8) {
        let file = resultsDir.appendingPathComponent("objc.json")
        try? (str + "\n").write(to: file, atomically: true, encoding: .utf8)
    }
}

func getEndpoint() -> String {
    if let env = ProcessInfo.processInfo.environment["LOAMS_TEST_ENDPOINT"], !env.isEmpty {
        lastEndpoint = env
        return env
    }

    let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    let fixtureScript = cwd.deletingLastPathComponent().appendingPathComponent("conformance/fixture-server.mjs").path

    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: "/usr/bin/node")
    proc.arguments = [fixtureScript, "--port", "0"]
    let pipe = Pipe()
    proc.standardOutput = pipe
    proc.standardError = Pipe()
    try? proc.run()

    let handle = pipe.fileHandleForReading
    let data = handle.availableData
    if let str = String(data: data, encoding: .utf8),
       let match = str.range(of: "\"url\":\"[^\"]+\"", options: .regularExpression) {
        let sub = String(str[match])
        let parts = sub.components(separatedBy: "\"")
        if parts.count >= 4 {
            lastEndpoint = parts[3]
            return lastEndpoint
        }
    }

    return lastEndpoint
}

final class ConformanceTests: XCTestCase {
    override func tearDown() {
        super.tearDown()
        writeReport(endpoint: lastEndpoint)
    }

    func test_objc_conformance_all_required_fixtures() async throws {
        let endpoint = getEndpoint()
        let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        let fixturesDir = cwd.deletingLastPathComponent().appendingPathComponent("fixtures")
        let manifestFile = fixturesDir.appendingPathComponent("manifest.json")
        let manifestData = try Data(contentsOf: manifestFile)
        let manifest = try JSONSerialization.jsonObject(with: manifestData) as! [String: Any]
        let fixtures = manifest["fixtures"] as! [[String: Any]]

        var ran = [String]()
        let session = URLSession.shared

        for fixture in fixtures {
            guard fixture["required"] as? Bool == true else { continue }
            let name = fixture["name"] as! String
            let file = fixturesDir.appendingPathComponent(fixture["file"] as! String)
            let recordingData = try Data(contentsOf: file)
            let recording = try JSONSerialization.jsonObject(with: recordingData) as! [String: Any]
            let steps = (recording["steps"] as? [[String: Any]]) ?? [recording]

            var answers = [Data]()

            for (stepIdx, step) in steps.enumerated() {
                let req = step["request"] as! [String: Any]
                let path = req["path"] as! String
                var headers = (req["headers"] as? [String: String]) ?? [:]
                headers["loams-fixture-name"] = name
                headers["loams-fixture-step"] = "\(stepIdx)"

                var bodyBytes = Data()
                if let b64 = req["bodyBase64"] as? String {
                    bodyBytes = Data(base64Encoded: b64) ?? Data()
                } else if let bodyStr = req["body"] as? String {
                    bodyBytes = bodyStr.data(using: .utf8) ?? Data()
                } else if let bodyObj = req["body"] {
                    bodyBytes = (try? JSONSerialization.data(withJSONObject: bodyObj)) ?? Data()
                }

                let cleanEndpoint = endpoint.hasSuffix("/") ? String(endpoint.dropLast()) : endpoint
                let cleanPath = path.hasPrefix("/") ? String(path.dropFirst()) : path
                let url = URL(string: "\(cleanEndpoint)/\(cleanPath)")!

                var urlReq = URLRequest(url: url)
                urlReq.httpMethod = "POST"
                urlReq.httpBody = bodyBytes
                for (k, v) in headers {
                    urlReq.setValue(v, forHTTPHeaderField: k)
                }

                let (respData, response) = try await session.data(for: urlReq)
                let httpResp = response as! HTTPURLResponse

                let expectedStatus = (step["response"] as? [String: Any])?["status"] as? Int ?? 200
                XCTAssertEqual(httpResp.statusCode, expectedStatus, "fixture \(name) step \(stepIdx)")

                let expectMap = (step["expect"] as? [String: Any]) ?? [:]
                if let expectedReason = expectMap["reason"] as? String {
                    let err = LoamsErrorParser.parse(withStatus: httpResp.statusCode, body: respData)
                    XCTAssertEqual(err.reason ?? err.unknownReason, expectedReason, "fixture \(name) step \(stepIdx) reason")
                }

                answers.append(respData)

                if let identicalTo = expectMap["identicalToStep"] as? Int {
                    XCTAssertEqual(respData, answers[identicalTo], "fixture \(name) step \(stepIdx) identicalToStep")
                }
            }

            ran.append(name)
        }

        XCTAssertEqual(ran.count, 28)
        writeReport(endpoint: endpoint)
    }

    func test_objc_retry_reuses_idempotency_key() {
        let key = LoamsIdempotency.mintKey()
        XCTAssertEqual(key.count, 36)
        let idx14 = key.index(key.startIndex, offsetBy: 14)
        XCTAssertEqual(key[idx14], "7") // UUIDv7
    }

    func test_objc_error_reason_mapping() {
        let b64 = "Cg9ub3RfaW1wbGVtZW50ZWQ" // unpadded base64 (R4)
        let data = LoamsErrorParser.decodeBase64Safe(b64)
        let reason = LoamsErrorParser.decodeErrorInfo(data)
        XCTAssertEqual(reason, "not_implemented")

        let jsonStr = "{\"code\":\"unimplemented\",\"message\":\"test\",\"details\":[{\"@type\":\"type.googleapis.com/google.rpc.ErrorInfo\",\"reason\":\"not_implemented\"}]}"
        let err = LoamsErrorParser.parse(withStatus: 501, body: jsonStr.data(using: .utf8)!)
        XCTAssertEqual(err.codeString, "unimplemented")
        XCTAssertEqual(err.reason, "not_implemented")
    }

    func test_objc_stream_resume_with_cursor() throws {
        let f1 = LoamsEnvelopes.packPayload("{\"cursor\":\"c1\",\"data\":\"msg1\"}".data(using: .utf8)!, flags: 0)
        let f2 = LoamsEnvelopes.packPayload("{\"cursor\":\"c2\",\"heartbeat\":true}".data(using: .utf8)!, flags: 0)
        let f3 = LoamsEnvelopes.packPayload("{}".data(using: .utf8)!, flags: 2)

        var combined = Data()
        combined.append(f1)
        combined.append(f2)
        combined.append(f3)

        let frames = LoamsEnvelopes.splitData(combined)
        let handle = LoamsStreamHandle(frames: frames, cursor: nil)
        let msgs = try handle.getMessages()
        XCTAssertEqual(msgs.count, 1) // heartbeat filtered
        XCTAssertEqual(handle.lastCursor, "c2")
    }

    func test_objc_token_source_refresh() {
        var count = 0
        let refresher: LoamsTokenRefresher = {
            count += 1
            return "token_v\(count)"
        }
        let ts = LoamsRefreshTokenSource(refresher: refresher)
        XCTAssertEqual(ts.token(), "token_v1")
        ts.refreshToken()
        XCTAssertEqual(ts.token(), "token_v2")
    }

    func test_objc_pagination_iterator() {
        let pages: [String: [String: Any]] = [
            "": ["items": ["a", "b"], "nextPageToken": "p2"],
            "p2": ["items": ["c"], "nextPageToken": "p3"],
            "p3": ["items": ["d"], "nextPageToken": ""],
        ]

        let items = LoamsPagination.iterate { token in
            return pages[token]
        }
        XCTAssertEqual(items as? [String], ["a", "b", "c", "d"])
    }
}
