// Consistency tokens and the session store (design §44 §7.4, D609; runtime
// contract R4 as amended by SDKP §2.1).
//
// The encoding is `crates/loams-collection/src/token.rs`'s:
//
//     v1:s<stream>/p<partition>@<offset>,…
//
// one entry per (stream, partition), sorted, with `merge` keeping the max offset
// per pair. `v1:` alone is the empty token. Every number is canonical decimal (no
// sign, no leading zero), a partition fits in 32 bits and a stream and offset in
// 64, and anything else is `invalid_argument` raised here, before the network.

using System.Collections.Frozen;
using System.Globalization;
using System.Text;

namespace Loams;

/// <summary>One (stream, partition, offset) entry of a <see cref="ConsistencyToken"/>.</summary>
public readonly record struct TokenPosition(ulong Stream, uint Partition, ulong Offset);

/// <summary>
/// A parsed consistency token: the log positions a write reached, which a read can
/// wait for (<c>Consistency.at_least</c>).
/// </summary>
public sealed class ConsistencyToken : IEquatable<ConsistencyToken>
{
    private const string Prefix = "v1:";

    /// <summary>The empty token, <c>v1:</c>.</summary>
    public static ConsistencyToken Empty { get; } = new([]);

    private ConsistencyToken(IReadOnlyList<TokenPosition> positions) => Positions = positions;

    /// <summary>The entries, sorted by stream then partition, one per pair.</summary>
    public IReadOnlyList<TokenPosition> Positions { get; }

    /// <summary>A token over these positions, normalised (sorted, max offset per pair).</summary>
    public static ConsistencyToken Of(IEnumerable<TokenPosition> positions)
    {
        ArgumentNullException.ThrowIfNull(positions);
        var sorted = positions
            .OrderBy((p) => p.Stream).ThenBy((p) => p.Partition).ThenBy((p) => p.Offset)
            .ToList();
        var normal = new List<TokenPosition>(sorted.Count);
        foreach (var position in sorted)
        {
            if (normal.Count > 0 && normal[^1].Stream == position.Stream && normal[^1].Partition == position.Partition)
            {
                normal[^1] = position;
            }
            else
            {
                normal.Add(position);
            }
        }
        return new ConsistencyToken(normal);
    }

    /// <summary>Parses a token.</summary>
    /// <exception cref="InvalidArgumentError">The text is not a <c>v1:</c> token.</exception>
    public static ConsistencyToken Parse(string text) =>
        TryParse(text, out var token)
            ? token
            : throw new InvalidArgumentError(string.Empty, Reason.InvalidArgument, "invalid_consistency_token",
                new Dictionary<string, string> { ["token"] = text ?? string.Empty },
                message: $"invalid consistency token \"{text}\": expected v1:s<stream>/p<partition>@<offset>,…");

    /// <summary>Parses a token, or answers false.</summary>
    public static bool TryParse(string? text, out ConsistencyToken token)
    {
        token = Empty;
        if (text is null || !text.StartsWith(Prefix, StringComparison.Ordinal))
        {
            return false;
        }
        var body = text[Prefix.Length..];
        if (body.Length == 0)
        {
            return true;
        }
        var positions = new List<TokenPosition>();
        foreach (var item in body.Split(','))
        {
            if (!TryItem(item, out var position))
            {
                return false;
            }
            positions.Add(position);
        }
        token = new ConsistencyToken(positions);
        return true;
    }

    private static bool TryItem(string item, out TokenPosition position)
    {
        position = default;
        if (!item.StartsWith('s'))
        {
            return false;
        }
        var slash = item.IndexOf("/p", StringComparison.Ordinal);
        if (slash < 0)
        {
            return false;
        }
        var at = item.IndexOf('@', slash);
        if (at < 0)
        {
            return false;
        }
        if (!Canonical(item[1..slash], out var stream)
            || !Canonical(item[(slash + 2)..at], out var partition)
            || !Canonical(item[(at + 1)..], out var offset)
            || partition > uint.MaxValue)
        {
            return false;
        }
        position = new TokenPosition(stream, (uint)partition, offset);
        return true;
    }

    private static bool Canonical(string digits, out ulong value)
    {
        value = 0;
        if (digits.Length == 0 || (digits.Length > 1 && digits[0] == '0'))
        {
            return false;
        }
        foreach (var c in digits)
        {
            if (c is < '0' or > '9')
            {
                return false;
            }
        }
        return ulong.TryParse(digits, NumberStyles.None, CultureInfo.InvariantCulture, out value);
    }

    /// <summary>The union of two tokens, keeping the max offset per (stream, partition).</summary>
    public ConsistencyToken Merge(ConsistencyToken other)
    {
        ArgumentNullException.ThrowIfNull(other);
        return Of(Positions.Concat(other.Positions));
    }

    /// <summary>The offset this token holds for a pair, or null.</summary>
    public ulong? Offset(ulong stream, uint partition)
    {
        ulong? best = null;
        foreach (var p in Positions)
        {
            if (p.Stream == stream && p.Partition == partition && (best is null || p.Offset > best))
            {
                best = p.Offset;
            }
        }
        return best;
    }

    /// <summary>The wire form.</summary>
    public override string ToString()
    {
        var text = new StringBuilder(Prefix);
        for (var i = 0; i < Positions.Count; i++)
        {
            if (i > 0)
            {
                text.Append(',');
            }
            var p = Positions[i];
            text.Append(CultureInfo.InvariantCulture, $"s{p.Stream}/p{p.Partition}@{p.Offset}");
        }
        return text.ToString();
    }

    public bool Equals(ConsistencyToken? other) =>
        other is not null && Positions.SequenceEqual(other.Positions);

    public override bool Equals(object? obj) => Equals(obj as ConsistencyToken);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var p in Positions)
        {
            hash.Add(p);
        }
        return hash.ToHashCode();
    }
}

/// <summary>
/// The session consistency token store (design §44 §7.4, D609; runtime contract R4).
/// </summary>
/// <remarks>
/// **Off by default.** Turn it on with <see cref="LoamsClientOptions.SessionConsistency"/>.
/// Every <c>consistency_token</c>, <c>token</c> or <c>read_token</c> a response
/// carries, and the <c>loams-consistency-token</c> response header, merge in, and
/// every read whose request has a <c>consistency</c> field the caller left unset
/// goes out with <c>at_least</c> = the session's token: read-your-writes in one
/// process. A token that is not a <c>v1:</c> token (a graph token, say) is kept
/// aside rather than merged and never sent as <c>at_least</c>.
/// </remarks>
public sealed class ConsistencySession
{
    private readonly object _gate = new();
    private ConsistencyToken? _token;

    /// <summary>The merged token later reads go out with, or null when none has been recorded.</summary>
    public string? Current
    {
        get
        {
            lock (_gate)
            {
                return _token?.ToString();
            }
        }
    }

    /// <summary>The merged token, parsed.</summary>
    public ConsistencyToken? Token
    {
        get
        {
            lock (_gate)
            {
                return _token;
            }
        }
    }

    /// <summary>How many tokens this session has merged.</summary>
    public int Recorded { get; private set; }

    /// <summary>How many tokens were not <c>v1:</c> tokens and were not merged.</summary>
    public int Unmergeable { get; private set; }

    /// <summary>Folds a response's token in. Empty is ignored; a foreign encoding is counted and skipped.</summary>
    public void Merge(string? token)
    {
        if (string.IsNullOrEmpty(token))
        {
            return;
        }
        lock (_gate)
        {
            if (!ConsistencyToken.TryParse(token, out var parsed))
            {
                Unmergeable++;
                return;
            }
            _token = _token is null ? parsed : _token.Merge(parsed);
            Recorded++;
        }
    }

    /// <summary>Forgets every token.</summary>
    public void Reset()
    {
        lock (_gate)
        {
            _token = null;
            Recorded = 0;
            Unmergeable = 0;
        }
    }

    /// <summary>
    /// The token a read goes out with: an explicit one wins, otherwise the session's.
    /// </summary>
    public static string? Resolve(string? explicitToken, ConsistencySession? session) =>
        !string.IsNullOrEmpty(explicitToken) ? explicitToken : session?.Current;

    /// <summary>The header a consistency token travels in.</summary>
    public const string Header = "loams-consistency-token";

    /// <summary>The header names read as well: the empty string means <c>STRONG</c>.</summary>
    public static readonly IReadOnlyDictionary<string, string> StrongRead =
        FrozenDictionary<string, string>.Empty;
}
