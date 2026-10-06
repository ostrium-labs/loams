// Retry classes and backoff (design §44 §7.4, D610; runtime contract R2).
//
// The numbers are M1.6 Ruling 5's and are the same in every SDK: base 100 ms,
// doubling, capped at 2 s, 3 retries, **full** jitter. Full jitter rather than
// exponential backoff alone, because every client retrying at the same instant
// after a node restart is how a recovering node gets knocked over again.
//
// Cancellation is part of the policy rather than something the loop happens to
// notice. In C# a caller who cancelled has already stopped wanting the answer, so
// `Task.Delay` is given the token and a retry is never started once it is
// cancelled; a caller's deadline therefore covers **the whole call**, retries and
// backoff included, which is what a deadline means.

namespace Loams;

/// <summary>The retry classes and their numbers, and the jittered backoff they wait.</summary>
public static class RetryPolicy
{
    /// <summary>The first backoff, and the multiplier's base, in milliseconds.</summary>
    public const int BaseDelayMs = 100;

    /// <summary>The ceiling on one backoff, in milliseconds.</summary>
    public const int MaxDelayMs = 2000;

    /// <summary>The retries after the first attempt, when nothing overrides it.</summary>
    public const int DefaultMaxRetries = 3;

    /// <summary>
    /// The ceiling on a server-sent <c>RetryInfo.retry_delay</c>, in
    /// milliseconds.
    /// </summary>
    /// <remarks>
    /// No proto carries <c>RetryInfo</c> today (R2), so nothing reaches this; the
    /// hook is here so the day one does, the number is already written down
    /// rather than chosen under pressure.
    /// </remarks>
    public const int MaxServerDelayMs = 30_000;

    /// <summary>
    /// The codes a retry may answer (D610).
    /// </summary>
    /// <remarks>
    /// <c>unavailable</c>, <c>deadline_exceeded</c> and
    /// <c>resource_exhausted</c>, and nothing else. Notably <b>not</b>
    /// <c>internal</c>: a server that answered with an internal error has already
    /// run the handler, and for a mutation that means the write may have happened,
    /// so repeating it on the SDK's own initiative is how one logical call becomes
    /// two writes. A mutation becomes retryable when it carries an idempotency
    /// key, which is a different question and is asked per call.
    /// </remarks>
    public static bool IsRetryableCode(Code code) =>
        code is Code.Unavailable or Code.DeadlineExceeded or Code.ResourceExhausted;

    /// <summary>
    /// The wait before retry number <paramref name="attempt"/> (zero for the first
    /// retry), with full jitter: uniform over
    /// <c>[0, min(cap, base × 2^attempt)]</c>.
    /// </summary>
    /// <param name="attempt">Zero-based: 0 is the wait before the first retry.</param>
    /// <param name="serverDelayMs">
    /// A server-sent <c>RetryInfo.retry_delay</c> in milliseconds, or 0. It
    /// replaces the computed backoff, capped at <see cref="MaxServerDelayMs"/>.
    /// </param>
    /// <param name="random">The jitter source, so a test can pin the draw.</param>
    public static TimeSpan Backoff(int attempt, int serverDelayMs = 0, Random? random = null)
    {
        if (serverDelayMs > 0)
        {
            return TimeSpan.FromMilliseconds(Math.Min(serverDelayMs, MaxServerDelayMs));
        }
        if (attempt < 0)
        {
            attempt = 0;
        }

        var ceiling = MaxDelayMs;
        // The shift is guarded rather than trusted: attempt is a caller's int, and
        // `100 << 40` is zero in C# rather than an overflow, which would silently
        // turn a long wait into no wait.
        if (attempt < 16)
        {
            var grown = BaseDelayMs << attempt;
            if (grown < ceiling)
            {
                ceiling = grown;
            }
        }

        var source = random ?? SharedRandom;
        // `Next(ceiling)` is exclusive at the top, so full jitter's half-open
        // [0, ceiling) is exactly it. `ceiling` is never 0: MaxDelayMs is 2000 and
        // `attempt < 16` only ever lowers the value.
        return TimeSpan.FromMilliseconds(source.Next(ceiling));
    }

    private static readonly Random SharedRandom = Random.Shared;

    /// <summary>
    /// Whether one more attempt is allowed.
    /// </summary>
    /// <param name="attempt">The zero-based attempt that just failed.</param>
    /// <param name="maxRetries">The retries after the first attempt.</param>
    /// <param name="retrySafe">
    /// The call's class: <see cref="RetryClass.Safe"/> for reads and idempotent
    /// RPCs, <see cref="RetryClass.Manual"/> for a mutation. A mutation is
    /// retryable once it carries an idempotency key, because the key is what makes
    /// the repeat safe — which is <see cref="Idempotency.Apply"/>'s decision, made
    /// before the first attempt.
    /// </param>
    /// <param name="code">The code the failure carried.</param>
    /// <param name="isCancelled">Whether the caller has already given up.</param>
    /// <remarks>
    /// A cancelled call is never retried whatever the class: the caller has said
    /// they do not want the answer, and a retry cannot change that. This is also
    /// why <paramref name="maxRetries"/> being negative is treated as none — a negative
    /// budget is a caller asking for no retries, not for unlimited ones, and
    /// "unlimited retries" is not a thing an SDK offers.
    /// </remarks>
    public static bool ShouldRetry(int attempt, int maxRetries, bool retrySafe, Code code, bool isCancelled)
    {
        if (isCancelled || maxRetries <= 0 || attempt >= maxRetries || !retrySafe)
        {
            return false;
        }
        return IsRetryableCode(code);
    }
}