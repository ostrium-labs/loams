// `documents.write_batch` (runtime contract R13, SDKP §2.3, design §44 §7.5).
//
// Any number of write ops, cut into chunks of at most `chunkRows` ops and about
// `chunkBytes` bytes, sent one chunk at a time. Each chunk gets its own
// idempotency key, minted once and reused on that chunk's retries, so a retried
// chunk is the same write. The tokens merge into one. The first chunk that fails
// stops the batch with a `BatchError` that says how many chunks were applied, so
// a caller can resume from there.

using Google.Protobuf;
using Loams.Collection.V1;

namespace Loams;

/// <summary>A finished batch: the merged token and how many chunks and ops it wrote.</summary>
/// <param name="Token">The merged consistency token of every chunk (empty when no chunk carried one).</param>
/// <param name="Chunks">How many chunks were applied.</param>
/// <param name="Ops">How many ops were applied.</param>
public sealed record BatchResult(string Token, int Chunks, int Ops);

/// <summary>
/// A batch that stopped at a failed chunk (reason <c>batch_partially_applied</c>,
/// client-side). <see cref="AppliedChunks"/> chunks and <see cref="AppliedOps"/> ops
/// went in before it; the failure is <see cref="Exception.InnerException"/>.
/// </summary>
public sealed class BatchError : LoamsError
{
    /// <summary>The client-side reason (docs/api/reasons.md).</summary>
    public const string BatchPartiallyApplied = "batch_partially_applied";

    public BatchError(int appliedChunks, int appliedOps, string token, LoamsError cause)
        : base(cause.Code, cause.Rpc, cause.Reason, BatchPartiallyApplied,
            new Dictionary<string, string>
            {
                ["applied_chunks"] = appliedChunks.ToString(System.Globalization.CultureInfo.InvariantCulture),
                ["applied_ops"] = appliedOps.ToString(System.Globalization.CultureInfo.InvariantCulture),
            },
            cause.Hint, cause.Detail, cause.HttpStatus, cause,
            $"write_batch stopped after {appliedChunks} applied chunk(s) ({appliedOps} ops): {cause.Message}")
    {
        AppliedChunks = appliedChunks;
        AppliedOps = appliedOps;
        Token = token;
    }

    /// <summary>The chunks applied before the failure.</summary>
    public int AppliedChunks { get; }

    /// <summary>The ops applied before the failure; resume from this index.</summary>
    public int AppliedOps { get; }

    /// <summary>The merged token of the applied chunks.</summary>
    public string Token { get; }
}

/// <summary>The chunking loop behind <c>WriteBatchAsync</c>.</summary>
public static class WriteBatch
{
    /// <summary>At most this many ops per chunk by default.</summary>
    public const int DefaultChunkRows = 10_000;

    /// <summary>About this many bytes per chunk by default (4 MiB).</summary>
    public const int DefaultChunkBytes = 4 * 1024 * 1024;

    /// <summary>Sends one chunk with its key.</summary>
    public delegate Task<WriteDocumentsResponse> ChunkSender(IReadOnlyList<WriteOp> chunk, string idempotencyKey,
        CancellationToken cancellationToken);

    /// <summary>Cuts <paramref name="ops"/> into chunks and sends them in order, one at a time.</summary>
    public static async Task<BatchResult> RunAsync(IEnumerable<WriteOp> ops, int chunkRows, int chunkBytes,
        ChunkSender send, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(ops);
        ArgumentNullException.ThrowIfNull(send);
        ArgumentOutOfRangeException.ThrowIfLessThan(chunkRows, 1);
        ArgumentOutOfRangeException.ThrowIfLessThan(chunkBytes, 1);

        ConsistencyToken? merged = null;
        string? foreign = null;
        var chunks = 0;
        var applied = 0;
        foreach (var chunk in Chunks(ops, chunkRows, chunkBytes))
        {
            cancellationToken.ThrowIfCancellationRequested();
            WriteDocumentsResponse answer;
            try
            {
                answer = await send(chunk, UuidV7.New(), cancellationToken).ConfigureAwait(false);
            }
            catch (LoamsError error)
            {
                throw new BatchError(chunks, applied, Print(merged, foreign), error);
            }
            if (answer.Token is { Length: > 0 } token)
            {
                if (ConsistencyToken.TryParse(token, out var parsed))
                {
                    merged = merged is null ? parsed : merged.Merge(parsed);
                }
                else
                {
                    foreign = token;
                }
            }
            chunks++;
            applied += chunk.Count;
        }
        return new BatchResult(Print(merged, foreign), chunks, applied);
    }

    private static string Print(ConsistencyToken? merged, string? foreign) =>
        merged?.ToString() ?? foreign ?? string.Empty;

    /// <summary>The chunks: at most <paramref name="rows"/> ops, and a new chunk once the next op would pass <paramref name="bytes"/>.</summary>
    public static IEnumerable<IReadOnlyList<WriteOp>> Chunks(IEnumerable<WriteOp> ops, int rows, int bytes)
    {
        var chunk = new List<WriteOp>();
        var size = 0;
        foreach (var op in ops)
        {
            var opSize = op.CalculateSize();
            if (chunk.Count > 0 && (chunk.Count >= rows || size + opSize > bytes))
            {
                yield return chunk;
                chunk = [];
                size = 0;
            }
            chunk.Add(op);
            size += opSize;
        }
        if (chunk.Count > 0)
        {
            yield return chunk;
        }
    }
}
