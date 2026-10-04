// A quickstart for the Swift SDK.
//
//     LOAMS_ENDPOINT=https://acme.loams.dev \
//     LOAMS_API_KEY=… \
//     swift run Quickstart
//
// It reads its configuration from the environment and **prints nothing that is a
// secret**: the plan's global constraint is "no secrets in examples", and an
// example that echoes the key is the most common way that constraint is broken.
//
// It exercises the four things an application actually does with the SDK, in the
// order it discovers them:
//
//   1. `getInstance` — who am I talking to, and what does it serve?
//   2. feature detection — is live sync available in *this* build variant?
//   3. the retrying, keyed mutation path — one key, reused on every attempt;
//   4. the stream — an `AsyncSequence` that resumes from a cursor.

import Foundation

#if canImport(FoundationNetworking)
// `URLSession` lives in FoundationNetworking on Linux and Foundation on Apple
// platforms. The plan's Task 4 row lists Linux as a target, so every file that
// names `URLSession` or `URLSessionConfiguration` needs this, not just the one
// that makes the request.
import FoundationNetworking
#endif
import Loams

@main
struct Quickstart {
    static func main() async {
        let environment = ProcessInfo.processInfo.environment

        guard let endpointText = environment["LOAMS_ENDPOINT"],
              let endpoint = URL(string: endpointText)
        else {
            print("set LOAMS_ENDPOINT to an instance, for example https://acme.loams.dev")
            exit(2)
        }

        let client: Loams
        do {
            client = try Loams(
                Options(
                    endpoint: endpoint,
                    // An API key does not expire, so R1's refresh is the no-op the
                    // contract describes. A person signed in through OIDC would
                    // pass an `OIDCExchange` here instead.
                    auth: APIKeyTokenSource(environment["LOAMS_API_KEY"] ?? ""),
                    transport: TransportConfig(endpoint: endpoint, protocol_: .connect, codec: .json)
                )
            )
        } catch {
            print("could not build a client: \(error)")
            exit(2)
        }

        // 1. What this instance is. Needs no credential, so it is the first call
        //    any client makes.
        do {
            let info = try await client.instance.getInstance()
            print("instance: \(info.name) \(info.serverVersion)")
            print("serves:   \(info.apiVersions.joined(separator: ", "))")
        } catch {
            print("getInstance failed: \(error)")
            exit(1)
        }

        // 2. Feature detection, **without calling**. `guard` raises the same error
        //    type a refused RPC would, so one `catch` covers both.
        do {
            try await client.system.guard("live")
            print("live sync: available")
        } catch let error as FeatureNotInVariantError {
            // R5: the branch is the **type**, never the package name (a proto
            // detail) and never the message.
            print("live sync: not in this variant (variant: \(error.variant))")
        } catch {
            print("feature detection failed: \(error)")
        }

        // 3. A mutation. The SDK mints one idempotency key before the first attempt
        //    and reuses it on every retry, so a retried write is one write.
        do {
            let request = DynamicMessage(
                protoTypeName: mutateRequestTypeName,
                fields: [
                    "collection": .string("acme"),
                    "statement": .string("insert into events (id) values (1)"),
                ]
            )
            let response = try await client.tables.mutate(request)
            print("mutate: \(response.fields)")
        } catch let error as FeatureNotInVariantError {
            print("live sync is not served here, so there is nothing to mutate: \(error.variant)")
        } catch {
            print("mutate failed: \(error)")
        }

        // 4. A stream. `for try await` is the whole interface: a failure throws
        //    out of the loop and a clean end returns, so there is no `Err()` after
        //    the loop that a caller can forget to check.
        //
        //    `loams.live.v1` is not served in the standard variant, so this reports
        //    the refusal — which is the point of showing it.
        do {
            let request = DynamicMessage(
                protoTypeName: watchRequestTypeName,
                fields: ["query_set": .string("acme")]
            )
            let stream = try await client.live.watch(request)
            var seen = 0
            for try await transition in stream.messages() {
                seen += 1
                print("transition: \(transition.fields)")
                if seen >= 10 { break }
            }
        } catch let error as FeatureNotInVariantError {
            print("watch is not served in this variant (variant: \(error.variant))")
        } catch {
            print("watch failed: \(error)")
        }
    }
}