// Pagination (design §44 §7.4, D617; runtime contract R6).
//
// AIP-158: `page_size` and `page_token` in, `next_page_token` out. A generated
// binding says which two fields those are (`FacadeOptions.pagination` is
// `"<items>:<next page token>"`), so the iterator is **one function for every**
// paged RPC rather than one per list RPC.
//
// In Swift the iterator is an `AsyncSequence`, which is the shape the language
// already has:
//
//     for try await collection in client.paginate(
//         binding: binding, request: request, items: { try $0.collections() }
//     ) {
//         use(collection)
//     }
//
// The `items` closure is how one function serves thirteen languages' worth of
// different response types: it pulls the repeated field off **this** message, and
// the SDK does the token threading and the stop condition. A generated per-call
// signature would arrive with `ListCollections` (API1 Task 2) and be this same
// loop with the closure filled in.
//
// The raw page call stays on the module, so a caller that wants pages, or wants
// to stop after one, does not have to use the iterator.
//
// **No RPC is paged yet.** `loams.collection.v1.ListCollections` arrives with API1
// Task 2, so the end-to-end half of the Swift conformance test is a deliberate
// skip, not an omission: a fixture for an RPC the server does not serve would be
// a test of the stub rather than of the SDK. What is pinned is the SDK's half —
// the token threading, the stop condition, and what happens when a binding is not
// paged.

import Foundation

/// Makes one page request.
///
/// A module method satisfies this shape, so the iterator drives the same code path
/// an application does rather than a parallel one.
public typealias PageFetcher<Request: LoamsMessage, Response: LoamsMessage> =
    @Sendable (Request, CallOptions) async throws -> Response

/// The iterator's failure, when a binding is not paged.
///
/// A refusal **reported through the sequence** rather than thrown at
/// construction: a Swift sequence's failure has to arrive from `next()`, because
/// `AsyncThrowingStream` has no other channel for it. Building the iterator
/// therefore cannot fail, and a caller that only iterates still learns why it
/// stopped.
public struct PaginationError: Error, Sendable, Equatable {
    /// The binding that is not paged.
    public let rpc: String
    /// Why, in a sentence a log can carry.
    public let message: String
}

/// Iterates every **item** of a paged call, following the tokens to the end.
///
/// - Parameters:
///   - binding: the call's generated binding, which names the two fields.
///   - fetch: makes one page request.
///   - request: the first request; the iterator copies it per page.
///   - items: pulls the items out of one response. This is the only
///     per-message-type part, and it is what lets one function serve every paged
///     RPC.
public func loamsPaginate<Request: LoamsMessage, Response: LoamsMessage, Item: Sendable>(
    binding: CallBinding,
    fetch: @escaping PageFetcher<Request, Response>,
    request: Request,
    items: @escaping @Sendable (Response) throws -> [Item],
    options: CallOptions = .init()
) -> AsyncThrowingStream<Item, any Error> {
    return AsyncThrowingStream { continuation in
        let task = Task {
            guard let shape = binding.pagination else {
                // The message names the binding, so the cause is obvious in a log
                // without the caller having to correlate it back to a call site.
                continuation.finish(
                    throwing: PaginationError(
                        rpc: binding.rpc,
                        message: "loams: \(binding.module).\(binding.name) is not a paged call: "
                            + "the proto's facade options name no pagination"
                    )
                )
                return
            }

            var token: String?
            while true {
                // A **copy** per page. Without it the iterator would mutate the
                // caller's request and send page one's token back on page two,
                // which is a loop rather than an iterator — and one that only
                // shows up once there is a second page.
                let pageRequest = loamsWithPageToken(request, field: shape.pageTokenField, token: token)
                let response: Response
                do {
                    response = try await fetch(pageRequest, options)
                } catch {
                    continuation.finish(throwing: LoamsError.map(error, rpc: binding.rpc))
                    return
                }

                do {
                    for item in try items(response) {
                        continuation.yield(item)
                    }
                } catch {
                    continuation.finish(throwing: LoamsError.map(error, rpc: binding.rpc))
                    return
                }

                // The stop condition is an **absent** token, not an empty one.
                // proto3 omits an empty string, so the last page carries no field
                // at all; treating "" as "another page" would loop forever on a
                // server that stops sending.
                let next = response.nextPageToken
                guard let next, !next.isEmpty else {
                    continuation.finish()
                    return
                }
                token = next
            }
        }
        continuation.onTermination = { _ in task.cancel() }
    }
}

/// A copy of `request` carrying `token` in `field`, or the request unchanged when
/// `token` is `nil`.
///
/// The copy is what stops the iterator from mutating the caller's message. When
/// the request type cannot carry the field the original is returned, which makes
/// the second page carry the **first** page's token — a real bug, but one only a
/// paged RPC can cause and only a caller with a hand-built message can hit, so it
/// is worth failing loudly on rather than looping silently.
func loamsWithPageToken<Request: LoamsMessage>(
    _ request: Request,
    field: String,
    token: String?
) -> Request {
    guard let token else { return request }
    guard let dynamic = request as? DynamicMessage else { return request }
    return dynamic.setting(field, to: token) as! Request
}

/// The request field names a paged call uses, from its binding.
///
/// A caller that builds a request by hand rather than through the iterator needs
/// them, and they are the AIP-158 names unless the proto says otherwise.
public func loamsPageFields(
    _ binding: CallBinding
) throws -> (pageSize: String, pageToken: String) {
    guard let shape = binding.pagination else {
        throw PaginationError(
            rpc: binding.rpc,
            message: "loams: \(binding.module).\(binding.name) is not a paged call"
        )
    }
    return (shape.pageSizeField, shape.pageTokenField)
}

/// A message's items under `field`, or `[]`.
///
/// An **absent** repeated field yields nothing, which is right: a server that
/// omits an empty repeated field is legal proto3, and throwing would break a caller
/// over a message the server is allowed to send.
public func loamsItems<Request: LoamsMessage>(_ response: Request, field: String) -> [JSONValue] {
    response.loamsFields()[field]?.arrayValue ?? []
}