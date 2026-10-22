// Client deadlines (runtime contract R12, SDKP §2.1).
//
// A unary call has one budget, 30 s unless the client or the call names another,
// and it covers the whole logical call: every attempt, every backoff. Each attempt
// tells the server what is left (`connect-timeout-ms` on Connect, `grpc-timeout` on
// gRPC and gRPC-Web), so the server can stop working on an answer nobody will read.
// When the budget runs out the call fails with `deadline_exceeded`, reason
// `client_deadline_exceeded`, and is not retried: there is no time left to retry in.

using System.Diagnostics;
using System.Globalization;

namespace Loams;

/// <summary>One logical call's budget.</summary>
public sealed class Deadline : IDisposable
{
    /// <summary>The client-side reason a spent budget carries (docs/api/reasons.md, client-side).</summary>
    public const string ClientDeadlineExceeded = "client_deadline_exceeded";

    private readonly CancellationTokenSource? _cts;
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private readonly TimeSpan _budget;
    private readonly CancellationToken _caller;

    private Deadline(TimeSpan budget, CancellationToken caller)
    {
        _budget = budget;
        _caller = caller;
        if (budget != System.Threading.Timeout.InfiniteTimeSpan && budget > TimeSpan.Zero)
        {
            _cts = CancellationTokenSource.CreateLinkedTokenSource(caller);
            _cts.CancelAfter(budget);
        }
    }

    /// <summary>Starts a budget. <see cref="System.Threading.Timeout.InfiniteTimeSpan"/> or zero means none.</summary>
    public static Deadline Start(TimeSpan budget, CancellationToken caller) => new(budget, caller);

    /// <summary>The token an attempt runs under: the caller's, cut off at the deadline.</summary>
    public CancellationToken Token => _cts?.Token ?? _caller;

    /// <summary>Whether the budget, and not the caller, ended the call.</summary>
    public bool Expired => _cts is { IsCancellationRequested: true } && !_caller.IsCancellationRequested;

    /// <summary>What is left of the budget, or null when there is none.</summary>
    public TimeSpan? Remaining
    {
        get
        {
            if (_cts is null)
            {
                return null;
            }
            var left = _budget - _clock.Elapsed;
            return left > TimeSpan.FromMilliseconds(1) ? left : TimeSpan.FromMilliseconds(1);
        }
    }

    /// <summary>The header that tells the server the remaining budget, per protocol.</summary>
    public static (string Name, string Value) Header(Protocol protocol, TimeSpan remaining)
    {
        var ms = Math.Max(1L, (long)Math.Ceiling(remaining.TotalMilliseconds));
        return protocol == Protocol.Connect
            ? ("connect-timeout-ms", ms.ToString(CultureInfo.InvariantCulture))
            : ("grpc-timeout", ms.ToString(CultureInfo.InvariantCulture) + "m");
    }

    /// <summary>The error a spent budget raises.</summary>
    public static DeadlineExceededError Exceeded(string rpc, TimeSpan budget, Exception? cause = null)
    {
        var ms = ((long)budget.TotalMilliseconds).ToString(CultureInfo.InvariantCulture);
        var message = $"{rpc}: deadline_exceeded ({ClientDeadlineExceeded}): the call did not finish in {ms} ms";
        if (cause is not null)
        {
            message += $"; the last attempt ended with {cause.GetType().Name}";
        }
        return new DeadlineExceededError(rpc, Reason.DeadlineExceeded, ClientDeadlineExceeded,
            new Dictionary<string, string> { ["timeout_ms"] = ms }, message: message);
    }

    public void Dispose() => _cts?.Dispose();
}
