// Idempotency keys (design §44 §7.4, D610; runtime contract R3).
//
// A mutating call that carries an `idempotency_key` field is given one **per
// logical call**, before the first attempt, and **the same key goes out on every
// retry**. A key regenerated per attempt turns one write into two, which is the
// exact failure the field exists to prevent, and the corpus's
// `mock_state_idempotent_decide` is the recording of it: the same
// `DecideApproval` sent twice with the same key against a mock whose approval
// state really moved, answering identical bytes and leaving the approval at
// revision 2 the second time.
//
// # Why the schema, not the object
//
// Whether to key is asked of the request's **descriptor**, not of the object a
// caller happened to build. That matters for proto3 `optional`: `MutateRequest.
// idempotency_key` is `optional string`, so a caller who leaves it out sends no
// key at all, the mutation is not retryable, and an SDK that inspected the
// property would see a null and have to guess whether that means "not declared"
// or "not set". It also must not confuse `MutateRequest` (which has the field)
// with `DeployRequest` (which does not), or it would invent a field the schema
// does not declare — and a request with a field the server does not know is
// rejected.
//
// # UUIDv7 rather than a dependency
//
// An idempotency key has to be unique across every client that has ever talked to
// an instance **and** sort by creation time, so an operator can correlate one in
// a log. UUIDv4 is unique but unordered. `System.Guid.NewGuid()` is a v4 and is
// what a C# developer reaches for by default, which is exactly why it is not what
// this uses: thirty lines of the layout are better than a key an operator cannot
// read a time out of. See `UuidV7`.

using System.Reflection;
using System.Security.Cryptography;
using Google.Protobuf;

namespace Loams;

/// <summary>
/// Decides a mutating call's idempotency key, once per logical call, and mints
/// one when the caller supplied none.
/// </summary>
public static class Idempotency
{
    /// <summary>
    /// A request the runtime has decided to key, and whether it made that
    /// decision.
    /// </summary>
    /// <param name="Request">
    /// The message to send: a copy with the key set, or the original when
    /// nothing was set.
    /// </param>
    /// <param name="Keyed">Whether the message carries a key the retry policy may rely on.</param>
    public readonly record struct KeyedRequest(IMessage Request, bool Keyed);

    /// <summary>
    /// Whether a generated message type's schema declares <c>idempotency_key</c>.
    /// </summary>
    /// <remarks>
    /// Resolved from the generated descriptor rather than from a CLR property, so
    /// the answer is about the **schema**: a proto3 <c>optional string</c> and a
    /// plain <c>string</c> are the same field as far as this is concerned, and a
    /// type whose key field is not a string is not a key field at all.
    /// </remarks>
    public static bool SchemaHasKey(string clrTypeName)
    {
        var descriptor = Descriptors.Find(clrTypeName);
        var field = descriptor?.FindFieldByName("idempotency_key");
        return field is not null && field.FieldType == Google.Protobuf.Reflection.FieldType.String;
    }

    /// <summary>
    /// Sets the key on a message whose schema declares one, and says whether the
    /// result is keyed.
    /// </summary>
    /// <param name="request">The caller's message. Never mutated: see below.</param>
    /// <param name="supplied">
    /// The caller's own key, or empty to have one minted. Supplying one makes the
    /// retry yours rather than the SDK's, which is right when the key is what
    /// your own storage dedupes on. Ignored when <paramref name="mint"/> is false.
    /// </param>
    /// <param name="mint">
    /// Whether the SDK may put a key on this call at all. False means the request
    /// goes out exactly as the caller built it and the call is reported unkeyed —
    /// which is not the same answer as "this schema has no key field", and the two
    /// were previously conflated into one. A caller who has already deduped this
    /// operation elsewhere, or who is deliberately replaying a recorded request,
    /// needs the first; a message whose schema has no key field needs the second.
    /// Defaults to true so the common call stays <c>Apply(request, key)</c>.
    /// </param>
    /// <remarks>
    /// The caller's message is **not** mutated. A message the caller still holds
    /// is a message they might reuse for a second logical call, and stamping this
    /// call's key onto it would make two calls share one key — the same class of
    /// bug as regenerating a key per attempt, and harder to see. So the message is
    /// cloned first, which for a generated type means a round trip through its own
    /// serialization: a deep copy with no reflection, and cheap next to an RPC.
    ///
    /// The clone is skipped entirely on the <c>mint: false</c> and
    /// already-populated paths, which are the two that return the message without
    /// changing it.
    /// </remarks>
    public static KeyedRequest Apply(IMessage request, string supplied, bool mint = true)
    {
        ArgumentNullException.ThrowIfNull(request);

        // The schema decides whether a key is even possible. This is asked before
        // `mint`, because "this message cannot be keyed" and "this call must not
        // be keyed" are independent: a caller may pass either flag for a type with
        // no key field and both answers are "unkeyed", and neither should be
        // reported as an error.
        var field = request.Descriptor.FindFieldByName("idempotency_key");
        if (field is null || field.FieldType != Google.Protobuf.Reflection.FieldType.String)
        {
            // A message without the field is left exactly as the caller wrote it:
            // keying it would invent a field the schema does not declare.
            return new KeyedRequest(request, Keyed: false);
        }

        // A key the caller already wrote is honoured whatever `mint` says. They
        // set it deliberately — it is what their own storage dedupes on — so
        // stripping it to honour `mint: false` would silently change the meaning
        // of a call the caller has already made idempotent.
        var existing = field.Accessor.GetValue(request) as string;
        if (existing is { Length: > 0 })
        {
            return new KeyedRequest(request, Keyed: true);
        }

        if (!mint)
        {
            return new KeyedRequest(request, Keyed: false);
        }

        var clone = Clone(request);
        field.Accessor.SetValue(clone, supplied.Length > 0 ? supplied : UuidV7.New());
        return new KeyedRequest(clone, Keyed: true);
    }

    /// <summary>A deep copy of a generated message, without reflection.</summary>
    private static IMessage Clone(IMessage message) =>
        message.Descriptor.Parser.ParseFrom(message.ToByteArray());
}

/// <summary>
/// Resolves a generated message type's <c>MessageDescriptor</c> from its CLR
/// full name, for the places that hold a name rather than a type.
/// </summary>
/// <remarks>
/// A tiny cache, because the call is made once per binding at startup and once
/// per idempotency decision afterwards — and because <c>Descriptor.Base64Data</c>
/// reflection over an assembly's types is not something to do per request. The
/// dictionary is a plain one behind a lock rather than a
/// <c>ConcurrentDictionary</c>: the write happens once per distinct name, so the
/// read path is the overwhelmingly common one and a lock is cheaper than the
/// allocation.
/// </remarks>
internal static class Descriptors
{
    private static readonly Dictionary<string, Google.Protobuf.Reflection.MessageDescriptor?> Cache = new(StringComparer.Ordinal);
    private static readonly object Gate = new();

    /// <summary>The descriptor for a generated type's full name, or null when there is none.</summary>
    public static Google.Protobuf.Reflection.MessageDescriptor? Find(string clrTypeName)
    {
        lock (Gate)
        {
            if (Cache.TryGetValue(clrTypeName, out var cached))
            {
                return cached;
            }
        }

        var descriptor = Resolve(clrTypeName);

        lock (Gate)
        {
            Cache[clrTypeName] = descriptor;
        }
        return descriptor;
    }

    private static Google.Protobuf.Reflection.MessageDescriptor? Resolve(string clrTypeName)
    {
        // The last segment is the message's own name; everything before it is the
        // namespace, which is where the generated `*Reflection` holder lives.
        var segments = clrTypeName.Split('.');
        if (segments.Length < 3)
        {
            return null;
        }
        var wanted = Normalize(segments[^1]);
        var holderNamespace = string.Join('.', segments[..^1]);

        // The holder is named after the **file** (`loams/instance/v1/instance.proto`
        // becomes `InstanceReflection`), which has no relation to the package's last
        // component, so the holders are enumerated rather than spelled: one per proto
        // file, a dozen in all, and the descriptor lookup is a set walk.
        foreach (var type in typeof(LoamsClient).Assembly.GetTypes())
        {
            if (!type.Name.EndsWith("Reflection", StringComparison.Ordinal) ||
                !string.Equals(type.Namespace, holderNamespace, StringComparison.Ordinal))
            {
                continue;
            }
            if (type.GetProperty("Descriptor", BindingFlags.Public | BindingFlags.Static)
                    ?.GetValue(null) is not Google.Protobuf.Reflection.FileDescriptor file)
            {
                continue;
            }
            foreach (var message in file.MessageTypes)
            {
                if (Normalize(message.Name) == wanted)
                {
                    return message;
                }
            }
        }
        return null;
    }

    /// <summary>A name with case and underscores dropped, for a spelling-insensitive comparison.</summary>
    private static string Normalize(string name)
    {
        Span<char> buffer = stackalloc char[name.Length];
        var length = 0;
        foreach (var character in name)
        {
            if (character is '_' or '-')
            {
                continue;
            }
            buffer[length++] = char.ToLowerInvariant(character);
        }
        return new string(buffer[..length]);
    }

}

/// <summary>
/// UUIDv7 as the canonical lowercase hyphenated string: 48 bits of Unix
/// milliseconds, 4 bits of version, 12 bits of a counter within the millisecond,
/// 2 bits of variant, 62 random bits.
/// </summary>
/// <remarks>
/// Hand-written rather than taken from a package, for the same reason the Go SDK
/// writes its own: thirteen SDKs need the identical layout, and the alternative
/// is a dependency whose only job is 30 lines. The clock is
/// <see cref="DateTimeOffset.UtcNow"/> rather than a monotonic one because the
/// field is a wall-clock timestamp an operator reads out of a log, not a
/// duration.
/// </remarks>
public static class UuidV7
{
    private static long _lastMillis;
    private static int _counter;

    /// <summary>A fresh UUIDv7.</summary>
    public static string New()
    {
        Span<byte> bytes = stackalloc byte[16];
        RandomNumberGenerator.Fill(bytes);

        var millis = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();

        // Monotonic within this process: two keys minted in the same millisecond
        // would otherwise share their timestamp, which is legal for a UUID but
        // makes the sort order — the reason for choosing v7 — a coin flip.
        // Interlocked on the pair so the read-modify-write cannot interleave.
        while (true)
        {
            var last = Interlocked.Read(ref _lastMillis);
            if (millis <= last)
            {
                millis = last;
                break;
            }
            if (Interlocked.CompareExchange(ref _lastMillis, millis, last) == last)
            {
                break;
            }
        }

        var counter = Interlocked.Increment(ref _counter) & 0x0FFF;

        // Big-endian for the timestamp, read out with shifts rather than by
        // dividing: dividing keeps the fractional bits of the lower digits and
        // truncates the carry, which puts the wrong byte in.
        bytes[0] = (byte)(millis >> 40);
        bytes[1] = (byte)(millis >> 32);
        bytes[2] = (byte)(millis >> 24);
        bytes[3] = (byte)(millis >> 16);
        bytes[4] = (byte)(millis >> 8);
        bytes[5] = (byte)millis;

        bytes[6] = (byte)((counter >> 8) | 0x70); // version 7
        bytes[7] = (byte)counter;
        bytes[8] = (byte)((bytes[8] & 0x3F) | 0x80); // variant 10

        // Written group by group rather than by computing each index's offset: the
        // four groups are 8-4-4-12 hex digits, and the arithmetic that maps a byte
        // index to a character index is the kind of thing that is right until the
        // layout changes.
        var text = new char[36];
        Hex(bytes[..4], text, 0);
        text[8] = '-';
        Hex(bytes[4..6], text, 9);
        text[13] = '-';
        Hex(bytes[6..8], text, 14);
        text[18] = '-';
        Hex(bytes[8..10], text, 19);
        text[23] = '-';
        Hex(bytes[10..16], text, 24);
        return new string(text);
    }

    private const string HexDigits = "0123456789abcdef";

    private static void Hex(ReadOnlySpan<byte> bytes, Span<char> into, int at)
    {
        for (var index = 0; index < bytes.Length; index++)
        {
            into[at + index * 2] = HexDigits[bytes[index] >> 4];
            into[at + index * 2 + 1] = HexDigits[bytes[index] & 0x0F];
        }
    }

    /// <summary>
    /// The Unix milliseconds a UUIDv7 encodes, and <see langword="false"/> for
    /// anything else.
    /// </summary>
    /// <remarks>
    /// The timestamp is the first **twelve** hex digits, not eight: 48 bits, and
    /// milliseconds since the epoch use 41 of them. Reading eight digits returns a
    /// number around 2^25, which is January 1970; reading sixteen silently
    /// includes the version and the counter. Exposed because "is this key from
    /// before or after that incident" is the question an operator actually asks of
    /// one.
    /// </remarks>
    public static bool TryReadTime(string value, out DateTimeOffset time)
    {
        time = default;
        if (value.Length != 36 || value[8] != '-' || value[13] != '-' || value[18] != '-' || value[23] != '-')
        {
            return false;
        }
        if (value[14] != '7')
        {
            return false;
        }
        if (value[19] is not ('8' or '9' or 'a' or 'b'))
        {
            return false;
        }

        long millis = 0;
        for (var index = 0; index < 12; index++)
        {
            var at = index < 8 ? index : index + 1;
            var digit = HexValue(value[at]);
            if (digit < 0)
            {
                return false;
            }
            // `(long)(uint)digit` because `millis << 4` is a signed 64-bit value and
            // OR-ing a sign-extended int into it is a compiler warning about a bug
            // that does not exist here — digit is 0..15.
            millis = (millis << 4) | (long)(uint)digit;
        }
        time = DateTimeOffset.FromUnixTimeMilliseconds(millis);
        return true;
    }

    private static int HexValue(char character) => character switch
    {
        >= '0' and <= '9' => character - '0',
        >= 'a' and <= 'f' => character - 'a' + 10,
        _ => -1,
    };
}