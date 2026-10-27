// `documents.writeBatch` (runtime contract R13, SDKP §2.3).
//
// Any number of write ops, cut into chunks of at most `chunkRows` ops and about
// `chunkBytes` bytes, sent one chunk at a time. Each chunk gets its own
// idempotency key, minted once and reused on that chunk's retries. The tokens
// merge into one. The first chunk that fails stops the batch with a
// `BatchError` that says how many chunks and ops were applied.

import Foundation

/// At most this many ops per chunk by default.
public let loamsDefaultChunkRows = 10_000

/// About this many bytes per chunk by default (4 MiB).
public let loamsDefaultChunkBytes = 4 * 1024 * 1024

/// The client-side reason a stopped batch carries (docs/api/reasons.md).
public let loamsBatchPartiallyApplied = "batch_partially_applied"

/// A finished batch.
public struct BatchResult: Sendable, Equatable {
    /// The merged consistency token of every chunk.
    public let token: String
    public let chunks: Int
    public let ops: Int
}

/// A batch that stopped at a failed chunk (reason `batch_partially_applied`).
public struct BatchError: Error, Sendable {
    public let appliedChunks: Int
    public let appliedOps: Int
    /// The merged token of the applied chunks.
    public let token: String
    /// The chunk's failure.
    public let cause: LoamsError
    public var reason: String { loamsBatchPartiallyApplied }
}

/// The chunks: at most `rows` ops, and a new chunk once the next op would pass `bytes`.
public func loamsChunks(_ ops: [JSONValue], rows: Int, bytes: Int) -> [[JSONValue]] {
    var chunks: [[JSONValue]] = []
    var chunk: [JSONValue] = []
    var size = 0
    for op in ops {
        let opSize = (try? op.encoded().count) ?? 0
        if !chunk.isEmpty, chunk.count >= rows || size + opSize > bytes {
            chunks.append(chunk)
            chunk = []
            size = 0
        }
        chunk.append(op)
        size += opSize
    }
    if !chunk.isEmpty { chunks.append(chunk) }
    return chunks
}

/// The chunking loop behind `writeBatch`.
public func loamsWriteBatch(
    _ ops: [JSONValue],
    chunkRows: Int,
    chunkBytes: Int,
    send: (_ chunk: [JSONValue], _ idempotencyKey: String) async throws -> DynamicMessage
) async throws -> BatchResult {
    precondition(chunkRows > 0 && chunkBytes > 0, "loams: writeBatch chunks need at least one row and one byte")
    var merged: ConsistencyToken?
    var chunks = 0
    var applied = 0
    for chunk in loamsChunks(ops, rows: chunkRows, bytes: chunkBytes) {
        let answer: DynamicMessage
        do {
            answer = try await send(chunk, loamsUUIDv7())
        } catch let error as LoamsError {
            throw BatchError(appliedChunks: chunks, appliedOps: applied, token: merged?.description ?? "", cause: error)
        }
        if let text = answer.fields["token"]?.stringValue, let parsed = ConsistencyToken.tryParse(text) {
            merged = merged.map { $0.merge(parsed) } ?? parsed
        }
        chunks += 1
        applied += chunk.count
    }
    return BatchResult(token: merged?.description ?? "", chunks: chunks, ops: applied)
}
