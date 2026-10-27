// `streams.subscribe` (runtime contract R14, SDKP §2.5, SDKP-2).
//
// A client loop over unary `Fetch`, so it works on every transport and through
// every proxy: fetch from the next offset, yield what came back, and move the
// offset to the last record's offset + 1. An empty fetch backs off 100 ms,
// doubling to 1 s, and the first non-empty one resets it. `Offset` is always the
// next offset to fetch, so a caller that stops can resume from it. Cancelling the
// enumeration stops the loop.
//
// `client.Streams.SubscribeAsync` wires this to `loams.stream.v1.StreamService/
// Fetch` when that package is on the main port (SDKP P0c); until then the loop is
// public so a caller can drive it over any fetch.

using System.Runtime.CompilerServices;

namespace Loams;

/// <summary>One record a subscription yields.</summary>
/// <param name="Partition">The partition it was read from.</param>
/// <param name="Offset">Its offset in the partition.</param>
/// <param name="Value">Its bytes.</param>
/// <param name="TimestampMs">Its append time, in Unix milliseconds, when the server sent one.</param>
/// <param name="Key">Its key, when it was produced with one.</param>
public sealed record StreamRecord(uint Partition, ulong Offset, byte[] Value, ulong TimestampMs = 0, byte[]? Key = null)
{
    /// <summary>The value as UTF-8 text.</summary>
    public string Text => System.Text.Encoding.UTF8.GetString(Value);
}

/// <summary>Fetches the records of one partition from an offset.</summary>
public delegate Task<IReadOnlyList<StreamRecord>> StreamFetch(ulong offset, CancellationToken cancellationToken);

/// <summary>A resumable subscription to one partition.</summary>
public sealed class Subscription : IAsyncEnumerable<StreamRecord>
{
    /// <summary>The first backoff after an empty fetch.</summary>
    public static readonly TimeSpan InitialBackoff = TimeSpan.FromMilliseconds(100);

    /// <summary>The longest backoff.</summary>
    public static readonly TimeSpan MaxBackoff = TimeSpan.FromSeconds(1);

    private readonly StreamFetch _fetch;
    private readonly Func<TimeSpan, CancellationToken, Task> _delay;
    private ulong _offset;

    /// <summary>A subscription from <paramref name="fromOffset"/>.</summary>
    /// <param name="fetch">One fetch.</param>
    /// <param name="fromOffset">The first offset to read.</param>
    /// <param name="delay">The wait between empty fetches; <c>Task.Delay</c> unless a test replaces it.</param>
    public Subscription(StreamFetch fetch, ulong fromOffset, Func<TimeSpan, CancellationToken, Task>? delay = null)
    {
        _fetch = fetch ?? throw new ArgumentNullException(nameof(fetch));
        _offset = fromOffset;
        _delay = delay ?? Task.Delay;
    }

    /// <summary>The next offset to fetch: the last yielded record's offset + 1. Resume from it.</summary>
    public ulong Offset => Interlocked.Read(ref _offset);

    public async IAsyncEnumerator<StreamRecord> GetAsyncEnumerator(CancellationToken cancellationToken = default)
    {
        var backoff = InitialBackoff;
        while (!cancellationToken.IsCancellationRequested)
        {
            IReadOnlyList<StreamRecord> records;
            try
            {
                records = await _fetch(Offset, cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                yield break;
            }
            catch (LoamsError error) when (error.Code == Code.Cancelled && cancellationToken.IsCancellationRequested)
            {
                yield break;
            }

            // A page with nothing at or past the offset is an empty one: back off rather
            // than spin on records already yielded.
            var from = Offset;
            if (!records.Any((r) => r.Offset >= from))
            {
                try
                {
                    await _delay(backoff, cancellationToken).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    yield break;
                }
                backoff = backoff * 2 > MaxBackoff ? MaxBackoff : backoff * 2;
                continue;
            }

            backoff = InitialBackoff;
            foreach (var record in records)
            {
                if (record.Offset < Offset)
                {
                    continue;
                }
                Interlocked.Exchange(ref _offset, record.Offset + 1);
                yield return record;
                if (cancellationToken.IsCancellationRequested)
                {
                    yield break;
                }
            }
        }
    }
}
