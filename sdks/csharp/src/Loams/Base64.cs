// Base64, as the Connect and gRPC-Web error framing needs it.
//
// # Why this exists rather than `Convert.FromBase64String`
//
// A failed RPC carries its `loams.errors.v1.ErrorInfo` as a **base64** string in
// `details[].value`, and a gRPC-Web refusal carries the same detail inside
// `grpc-status-details-bin`, which is base64 of a `google.rpc.Status`. So base64
// appears in exactly one place in this SDK — the error path — and
// `Convert.FromBase64String` rejects the corpus.
//
// It rejects **unpadded** base64. `System.Convert` requires the input length to
// be a multiple of four; the corpus's `not_implemented` detail is recorded as
// `Cg9ub3RfaW1wbGVtZW50ZWQ`, twenty-three characters, which is valid standard
// base64 for `0a 0f` + `not_implemented` with the padding stripped. Seven of the
// ten distinct `details[].value` strings in `sdks/fixtures` are unpadded. So
// `Convert.FromBase64String` threw on most of the corpus's reasons, the throw was
// caught and turned into "this error has no reason", and every structured-reason
// fixture reported `reason: none` against a server that had said exactly which
// reason it meant. R8 is precisely about that reason reaching the caller.
//
// It is also a **crash** waiting to happen rather than only a lost reason:
// `WireReader.GrpcWebFailure` called it on `grpc-status-details-bin` with no
// guard, so a server that recorded that header unpadded would have thrown a
// `FormatException` out of the response reader instead of producing a refusal.
//
// # The rules
//
// The alphabet is the standard one (`A-Za-z0-9+/`, `=` padding), **not** the
// URL-safe variant, because that is what the wire uses. Three rules, and each is
// a way a reason goes missing without an error:
//
//   - whitespace is **skipped**, because HTTP header folding and a line-wrapped
//     `grpc-status-details-bin` both put it there and neither changes the value;
//   - padding **ends** the stream, and a partial final group of two or three
//     characters is accepted — that is the unpadded corpus. A final group of
//     **one** character is rejected: one leftover sextet has nothing to pair it
//     with, so there is no byte it could stand for;
//   - anything else outside the alphabet is **rejected** rather than skipped. A
//     decoder that quietly drops invalid characters turns a truncated detail into
//     a shorter but still-plausible one, which is how a `reason` goes missing
//     without an error — and it is why this cannot be "decode with
//     `Convert` after padding it".
//
// Padding is also validated rather than merely terminated: once a `=` has been
// seen, a further alphabet character makes the input **invalid**. The C++ SDK
// (`sdks/cpp/src/base64.cpp`) stops at the padding instead and ignores whatever
// follows; this is deliberately stricter, because the two only differ on input
// this corpus never produces and the strict answer is the one that does not hand
// a caller a truncated reason.

using System.Collections.Generic;

namespace Loams;

/// <summary>
/// The one base64 decoder, for the two places the error path needs one.
/// </summary>
public static class Base64
{
    /// <summary>
    /// Decodes standard base64, padded or not, or returns <see langword="null"/>
    /// for input this decoder will not accept.
    /// </summary>
    /// <remarks>
    /// Null rather than an empty array for a refusal, so "the value was empty" and
    /// "the value was not base64" stay distinguishable: an empty value decodes to
    /// an empty array and <c>null</c> means the server did not write base64 at
    /// all. Callers report both the same way — a refusal with no
    /// <c>ErrorInfo</c> — but a test can tell them apart, which is the point.
    /// </remarks>
    public static byte[]? Decode(string? text)
    {
        if (text is null)
        {
            return null;
        }

        var bytes = new List<byte>(text.Length / 4 * 3 + 3);
        Span<int> group = stackalloc int[4];
        var filled = 0;
        var padded = false;

        foreach (var character in text)
        {
            if (char.IsWhiteSpace(character))
            {
                continue;
            }
            if (character == '=')
            {
                // Padding ends the stream. Anything but whitespace after it is
                // rejected below, which is this decoder's one deliberate
                // divergence from the C++ SDK — see the file header.
                padded = true;
                continue;
            }
            if (padded)
            {
                return null;
            }

            var value = ValueOf(character);
            if (value < 0)
            {
                return null;
            }
            group[filled++] = value;
            if (filled == 4)
            {
                var packed = (group[0] << 18) | (group[1] << 12) | (group[2] << 6) | group[3];
                bytes.Add((byte)((packed >> 16) & 0xFF));
                bytes.Add((byte)((packed >> 8) & 0xFF));
                bytes.Add((byte)(packed & 0xFF));
                filled = 0;
            }
        }

        // One leftover character is one sextet with nothing to pair it with: there
        // is no byte it could stand for, so it is a malformed encoding rather than
        // a short one.
        if (filled == 1)
        {
            return null;
        }
        if (filled is 2 or 3)
        {
            var packed = (group[0] << 18) | (group[1] << 12) | (filled == 3 ? group[2] << 6 : 0);
            bytes.Add((byte)((packed >> 16) & 0xFF));
            if (filled == 3)
            {
                bytes.Add((byte)((packed >> 8) & 0xFF));
            }
        }
        return bytes.ToArray();
    }

    /// <summary>
    /// The value of a base64 character, or -1. Not a lookup indexed by
    /// <see cref="char"/>: an index by character is a table of 65 536 entries to
    /// read six values from.
    /// </summary>
    private static int ValueOf(char character) => character switch
    {
        >= 'A' and <= 'Z' => character - 'A',
        >= 'a' and <= 'z' => character - 'a' + 26,
        >= '0' and <= '9' => character - '0' + 52,
        '+' => 62,
        '/' => 63,
        _ => -1,
    };
}