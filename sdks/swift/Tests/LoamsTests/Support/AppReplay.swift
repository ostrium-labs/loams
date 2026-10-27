// The recorded app scenario (`sdks/fixtures/recorded/app/*.json`,
// `sdks/fixtures/app-scenario.md`), replayed through the public facade against
// the fixture server (SDKP P0d).
//
// Every step names its facade call and carries the recorded request and answer.
// The request goes through the client's invoker with that call's binding (the
// facade's own path: namespace, idempotency key, retries, errors) and the
// recorded body, and a step that names a builder case is first checked against
// this SDK's builder output, so the replay proves the builder too. The answer
// is compared with the recording, the fixture's `volatile` fields masked; a
// refusal is compared by its code.

import Foundation
import XCTest
@testable import Loams

/// `sdks/`, from this file's own path rather than the working directory.
let loamsSdksDirectory: URL = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()   // Support
    .deletingLastPathComponent()   // LoamsTests
    .deletingLastPathComponent()   // Tests
    .deletingLastPathComponent()   // swift
    .deletingLastPathComponent()   // sdks

/// `sdks/fixtures`.
let loamsFixturesDirectory: URL = loamsSdksDirectory.appendingPathComponent("fixtures")

/// Each builder corpus case's output from this SDK's builders, keyed by its
/// file name in `sdks/fixtures/builders`.
func builderCorpusCases() -> [String: JSONValue] {
    let unit = [1.0, 0.0, 0.0, 0.0]
    let hybrid = { (fusion: Fusion) -> JSONValue in
        .object(Search.on("notes").text("refund", fields: ["body"]).vector("embedding", unit, k: 5)
            .fuse(fusion).limit(3).json)
    }
    let page = Search.on("notes").text("refund", fields: ["body"]).filter(Filter.eq("tags", "policy"))
        .highlight("body").select(include: ["title"]).limit(2).sort(.score(), .pk())
    return [
        "schema_notes": Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", dim: 4).json,
        "filter_term": Filter.eq("tags", "policy").json,
        "filter_bool_nested": (Filter.eq("tags", "policy") && (Filter.range("n", gte: 2) || !Filter.exists("archived"))).json,
        "filter_range_open": Filter.range("n", gt: 1).json,
        "filter_ids": Filter.ids([1, "k-str", .uuid("0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e")]).json,
        "search_text_filter_highlight_select": .object(page.json),
        "search_after_page": .object(page.after([.number(0.3438858389854431), .int(3)]).json),
        "search_vector_ann_params": .object(Search.on("notes")
            .vector("embedding", unit, k: 3, params: ["nprobes": .int(8), "refineFactor": .int(2)]).limit(3).json),
        "search_hybrid_rrf": hybrid(.rrf(k: 60)),
        "search_hybrid_dbsf": hybrid(.dbsf),
        "search_hybrid_weighted": hybrid(.weighted([0.7, 0.3])),
        "search_expand": .object(Search.on("notes").text("refund", fields: ["body"])
            .expand(graph: "notes_graph", fromField: "id", hops: 1).json),
    ]
}

enum AppReplay {
    static func fixtureURL(_ name: String) -> URL {
        loamsFixturesDirectory.appendingPathComponent("recorded/app/\(name).json")
    }

    static func available(_ name: String) -> Bool {
        FileManager.default.fileExists(atPath: fixtureURL(name).path)
    }

    /// Runs every step of `name` against `endpoint`; answers the failures.
    static func replay(_ endpoint: URL, _ name: String) async throws -> [String] {
        guard case .object(let fixture) = try JSONValue.decoded(from: Data(contentsOf: fixtureURL(name))),
              case .array(let steps)? = fixture["steps"]
        else { return ["\(name): not a recording"] }
        let volatile = Set((fixture["volatile"]?.arrayValue ?? []).compactMap(\.stringValue))
        let client = try newTestClient(endpoint)
        let cases = builderCorpusCases()
        var failures: [String] = []
        for (index, stepValue) in steps.enumerated() {
            guard case .object(let step) = stepValue,
                  let call = step["call"]?.stringValue,
                  case .object(let response)? = step["response"]
            else { failures.append("\(name) step \(index): not a step"); continue }
            let label = "\(name) step \(index) (\(call))"
            let text = step["request"]?.objectValue?["body"]?.stringValue ?? ""
            let request = text.isEmpty ? [:] : (JSONValue.decoded(fromUTF8: text)?.objectValue ?? [:])
            if let builder = step["builder"]?.stringValue,
               let problem = checkBuilder(builder, request, cases) {
                failures.append("\(label): \(problem)")
            }
            guard let binding = binding(call) else {
                failures.append("\(label): the facade has no call \(call)")
                continue
            }
            let status = response["status"]?.intValue ?? 0
            let expected = response["body"] ?? .object([:])
            let options = CallOptions(
                maxRetries: 0,
                headers: ["loams-fixture-name": name, "loams-fixture-step": String(index)],
                // The recording carries its own key where the call is keyed; one the
                // SDK minted would not match the recorded body.
                mintIdempotencyKey: false
            )
            do {
                let answer: DynamicMessage = try await client.invoker.unary(
                    binding: binding,
                    request: DynamicMessage(protoTypeName: binding.package + "." + binding.method + "Request", fields: request),
                    overrides: options
                )
                if status != 200 {
                    failures.append("\(label): answered, but the recording is a "
                        + (expected.objectValue?["code"]?.stringValue ?? "\(status)"))
                    continue
                }
                if let diff = compare(mask(expected, volatile), mask(.object(answer.fields), volatile)) {
                    failures.append("\(label): \(diff)")
                }
            } catch {
                let code = errorCode(error)
                let want = expected.objectValue?["code"]?.stringValue ?? ""
                if status == 200 {
                    failures.append("\(label): refused (\(code)) where the recording answered: \(error)")
                } else if code != want {
                    failures.append("\(label): refused with \(code), the recording with \(want)")
                }
            }
        }
        return failures
    }

    /// `documents.write` to the `documents` module's `Write` binding.
    static func binding(_ call: String) -> CallBinding? {
        let parts = call.split(separator: ".", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { return nil }
        let pascal = parts[1].split(separator: "_").map { $0.prefix(1).uppercased() + $0.dropFirst() }.joined()
        return loamsBinding(module: parts[0], call: pascal)
    }

    static func errorCode(_ error: Error) -> String {
        switch error {
        case let e as LoamsError: return e.code.rawValue
        case let e as FeatureNotInVariantError: return e.base.code.rawValue
        case let e as TokenExpiredError: return e.base.code.rawValue
        default: return "\(error)"
        }
    }

    /// The builder case's output against the part of the request it builds.
    static func checkBuilder(_ name: String, _ request: [String: JSONValue], _ cases: [String: JSONValue]) -> String? {
        let path = loamsFixturesDirectory.appendingPathComponent("builders/\(name).json")
        guard let data = try? Data(contentsOf: path),
              case .object(let entry)? = try? JSONValue.decoded(from: data)
        else { return "no builder case \(name)" }
        guard let built = cases[name] else { return "the Swift builders have no case \(name)" }
        let part: JSONValue?
        switch entry["builder"]?.stringValue {
        case "schema": part = request["schema"]
        case "filter": part = request["where"] ?? request["filter"]
        default: part = .object(request.filter { $0.key != "namespace" })
        }
        guard let part else { return "the recording has no part for \(name)" }
        if let diff = compare(part, built) {
            return "the \(name) builder differs from the recording: \(diff)"
        }
        return nil
    }

    static func mask(_ value: JSONValue, _ volatile: Set<String>) -> JSONValue {
        switch value {
        case .object(let o):
            return .object(o.filter { !volatile.contains($0.key) }.mapValues { mask($0, volatile) })
        case .array(let a):
            return .array(a.map { mask($0, volatile) })
        default:
            return value
        }
    }

    static func number(_ value: JSONValue?) -> Double? {
        switch value {
        case .int(let i)?: return Double(i)
        case .number(let d)?: return d
        default: return nil
        }
    }

    /// nil when equal; numbers compare to ~1e-6 relative (the recording's float32s).
    static func compare(_ want: JSONValue?, _ got: JSONValue?, at: String = "$") -> String? {
        switch want {
        case .object(let w)?:
            guard case .object(let g)? = got else { return "\(at): \(describe(got)) is not an object" }
            for key in Set(w.keys).union(g.keys).sorted() {
                if let problem = compare(w[key], g[key], at: "\(at).\(key)") { return problem }
            }
            return nil
        case .array(let w)?:
            guard case .array(let g)? = got, g.count == w.count else {
                return "\(at): \(describe(got)) is not \(w.count) items"
            }
            for (i, item) in w.enumerated() {
                if let problem = compare(item, g[i], at: "\(at)[\(i)]") { return problem }
            }
            return nil
        case .int?, .number?:
            let w = number(want)!
            guard let g = number(got), abs(w - g) <= 1e-6 * max(1.0, abs(w)) else {
                return "\(at): \(describe(got)), recorded \(describe(want))"
            }
            return nil
        default:
            return want == got ? nil : "\(at): \(describe(got)), recorded \(describe(want))"
        }
    }

    static func describe(_ value: JSONValue?) -> String {
        guard let value, let data = try? value.encoded() else { return "nothing" }
        return String(decoding: data, as: UTF8.self)
    }

    /// Adds `names` to `sdks/fixtures/results/swift.json`'s `ran`.
    static func record(_ names: [String]) throws {
        let path = loamsFixturesDirectory.appendingPathComponent("results/swift.json")
        try FileManager.default.createDirectory(
            at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
        var report: [String: JSONValue] = [
            "language": .string("swift"), "transport": .string("connect"), "ran": .array([]),
        ]
        if let data = try? Data(contentsOf: path), let existing = JSONValue.decodedObject(from: data) {
            report.merge(existing) { _, new in new }
        }
        let ran = Set((report["ran"]?.arrayValue ?? []).compactMap(\.stringValue)).union(names)
        report["ran"] = .array(ran.sorted().map { .string($0) })
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        var data = try encoder.encode(JSONValue.object(report))
        data.append(0x0A)
        try data.write(to: path)
    }
}
