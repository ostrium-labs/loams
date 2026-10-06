// What a recording says must be true of the SDK's answer, and the reader for
// each of those facts.
//
// Split out of `CorpusDriver` so the driver is the thing that *replays* and this
// is the thing that *judges*: a failure in here is a disagreement about what the
// recording claims, and a failure in the driver is a disagreement about what got
// sent. One file with both makes a red run ambiguous.
//
// # Every field is read out of the descriptor, never off a generated property
//
// The corpus names response fields in proto3 JSON (`apiVersions`, `snapshotReset`)
// and the generated C# names are PascalCase, so the JSON name is the one the
// corpus and `fixture-server.mjs` agree on. That is also why an enum is compared
// **by name** and a 64-bit integer **as a string**: an enum is a number in memory
// and a name in the recording, so comparing the number would pass for every
// approval state in the proto and fail for every reason that matters.
//
// # A field named in `expect` may live one level down
//
// `mock_state_idempotent_decide` records `expect.state` and `expect.revision` for a
// `DecideApprovalResponse`, and neither is a field of that message — they are
// fields of the `approval` it wraps. A reader that looks only at the top level
// reports "the response has no state" against a response that plainly has one,
// which is how four required fixtures spent a whole wave red.
//
// So the lookup is: the response's own descriptor first, then the first singular
// message field of the response — in field-number order, which is the order the
// proto declares them — that declares the wanted name. That is derived from the
// response message, never from a list of wrapper field names, so a new response
// that wraps its subject differently is handled by the same rule.

using System.Text;
using System.Text.Json;
using Google.Protobuf;
using Google.Protobuf.Reflection;

namespace Loams.Tests;

/// <summary>
/// Checks one replayed step against the recording's own <c>expect</c> block.
/// </summary>
public static class Expectations
{
    /// <summary>
    /// The response fields a recording's <c>expect</c> block can name.
    /// </summary>
    /// <remarks>
    /// The whole set, and not "whatever the fixtures happen to use": a recording
    /// that named a field this list lacks would be silently unchecked, which is the
    /// one way a corpus driver can go green without testing anything.
    /// </remarks>
    private static readonly string[] ReadBackFields =
        ["apiVersions", "state", "revision", "cursor", "snapshotReset"];

    /// <summary>The two refusals R8 distinguishes, checked from the recording's own words.</summary>
    public static IEnumerable<string> CheckError(
        string fixtureName, int step, JsonElement expect, LoamsError error)
    {
        var problems = new List<string>();
        if (expect.ValueKind != JsonValueKind.Object)
        {
            return problems;
        }

        if (expect.TryGetProperty("reason", out var reason))
        {
            if (reason.ValueKind != JsonValueKind.String)
            {
                // `reason: null` is a recorded fact: the server sent no `ErrorInfo`
                // and the SDK must not invent one (R8, and `mock_status_unauthenticated`).
                if (error.Reason != Loams.Reason.None)
                {
                    problems.Add(
                        $"{fixtureName} step {step}: expect.reason is null, and the SDK reported " +
                        $"{Loams.ReasonRegistry.Name(error.Reason)}");
                }
            }
            else
            {
                var want = reason.GetString()!;
                var got = error.Reason == Loams.Reason.None
                    ? error.UnknownReason
                    : Loams.ReasonRegistry.Name(error.Reason);
                if (got != want)
                {
                    problems.Add(
                        $"{fixtureName} step {step}: expect.reason is {want}, and the SDK reported {got ?? "none"}");
                }
            }
        }

        if (expect.TryGetProperty("grpcStatus", out var grpcStatus))
        {
            var want = (Loams.Code)grpcStatus.GetInt32();
            if (error.Code != want)
            {
                problems.Add(
                    $"{fixtureName} step {step}: expect.grpcStatus is {grpcStatus.GetInt32()}, and the " +
                    $"SDK reported {error.Code}");
            }
        }
        return problems;
    }

    /// <summary>Checks a step the SDK answered rather than refused.</summary>
    /// <param name="status">The HTTP status the recording answers with.</param>
    /// <param name="response">What the SDK made of it, or null when it made nothing.</param>
    /// <param name="expected">
    /// Whether the recording answers with a message at all, which is decided from
    /// the recording's own HTTP status rather than from the driver's mood.
    /// </param>
    public static IEnumerable<string> CheckSuccess(
        string fixtureName,
        int step,
        int status,
        JsonElement expect,
        IMessage? response,
        bool expected)
    {
        var problems = new List<string>();
        if (!expected)
        {
            problems.Add(
                $"{fixtureName} step {step}: the recording answers HTTP {status} and the SDK " +
                "returned a message, so a refusal was read as a success");
            return problems;
        }
        if (response is null)
        {
            problems.Add(
                $"{fixtureName} step {step}: the recording answers HTTP {status} and the SDK returned nothing");
            return problems;
        }
        problems.AddRange(CheckMessage(fixtureName, step, expect, response));
        return problems;
    }

    /// <summary>The response fields the recording's <c>expect</c> names.</summary>
    public static IEnumerable<string> CheckMessage(
        string fixtureName, int step, JsonElement expect, IMessage message)
    {
        var problems = new List<string>();
        if (expect.ValueKind != JsonValueKind.Object)
        {
            return problems;
        }

        foreach (var name in ReadBackFields)
        {
            if (!expect.TryGetProperty(name, out var want))
            {
                continue;
            }
            var holder = Holder(message, name);
            if (holder is null)
            {
                problems.Add(
                    $"{fixtureName} step {step}: expect.{name} is set, and neither " +
                    $"{message.Descriptor.Name} nor any message it wraps declares a {name} field");
                continue;
            }
            if (want.ValueKind == JsonValueKind.Array)
            {
                // `apiVersions` is a repeated field, and the recording states which
                // packages must be **present**: the mock serves five and names one,
                // so containment is the check and equality would be wrong.
                var wanted = want.EnumerateArray().Select((item) => item.GetString()!).ToList();
                var actual = ReadRepeated(holder, name);
                foreach (var entry in wanted)
                {
                    if (!actual.Contains(entry))
                    {
                        problems.Add(
                            $"{fixtureName} step {step}: expect.{name} contains {entry}, and the " +
                            $"response has [{string.Join(", ", actual)}]");
                    }
                }
                continue;
            }
            var got = Render(holder, name);
            var claim = Wanted(want);
            if (got != claim)
            {
                problems.Add(
                    $"{fixtureName} step {step}: expect.{name} is {claim}, and the response says {got ?? "none"}");
            }
        }

        return problems;
    }

    /// <summary>
    /// The value a recording states, rendered the way <see cref="Render"/> renders
    /// the SDK's.
    /// </summary>
    /// <remarks>
    /// Almost nothing, and the one thing is a boolean. <c>JsonElement.ToString()</c>
    /// on a JSON <c>true</c> gives <c>True</c> — .NET's spelling, not the wire's — so
    /// comparing it against a rendered <c>true</c> fails on a case difference while
    /// reading like a disagreement about the value. A recording writes
    /// <c>snapshotReset: false</c> in lower case, and so does proto3 JSON.
    /// </remarks>
    private static string Wanted(JsonElement value) => value.ValueKind switch
    {
        JsonValueKind.True => "true",
        JsonValueKind.False => "false",
        JsonValueKind.Null or JsonValueKind.Undefined => string.Empty,
        _ => value.ToString(),
    };

    /// <summary>
    /// The message a named field lives on: this one, or the first singular message
    /// field it wraps that declares the name.
    /// </summary>
    private static IMessage? Holder(IMessage message, string jsonName)
    {
        if (Declares(message.Descriptor, jsonName))
        {
            return message;
        }
        foreach (var field in message.Descriptor.Fields.InFieldNumberOrder())
        {
            // A repeated or map field is a collection of messages rather than one
            // message this response is *about*, so descending into it would be
            // reading some other response's subject. Singular fields only.
            if (field.IsMap || field.IsRepeated || field.FieldType != FieldType.Message ||
                field.MessageType is null || !Declares(field.MessageType, jsonName))
            {
                continue;
            }
            // An unset singular message reads back as its default instance, which
            // answers "the wrapper was empty" as a field value rather than as a
            // missing field — the right answer, and a different one from what a
            // response that never declared the field gives.
            if (field.Accessor.GetValue(message) is IMessage nested)
            {
                return nested;
            }
        }
        return null;
    }

    /// <summary>Whether a message declares a field under its proto3 JSON name.</summary>
    private static bool Declares(MessageDescriptor descriptor, string jsonName) => Find(descriptor, jsonName) is not null;

    /// <summary>
    /// One field, rendered the way the recording renders it: an enum by name, a
    /// 64-bit integer as a string, a bool as <c>true</c>/<c>false</c>.
    /// </summary>
    /// <remarks>
    /// Null means "declared with presence, and unset" — a proto3 <c>optional</c>, a
    /// oneof case that is not this one, or a message field that was never set.
    ///
    /// A field with **implicit** presence has no such question to ask: protobuf's
    /// <c>SingleFieldAccessor.HasValue</c> throws <c>Presence is not implemented for
    /// this field</c> on one, so asking is not merely wrong but impossible. Its value
    /// is rendered either way, and a default compares as the default — which is the
    /// honest answer for an <c>expect.state</c> the response does not carry, and a
    /// far better one than reporting that the field is missing.
    /// </remarks>
    private static string? Render(IMessage message, string jsonName)
    {
        var field = Find(message.Descriptor, jsonName);
        if (field is null)
        {
            return null;
        }
        if (field.IsMap)
        {
            return string.Empty;
        }
        if (field.IsRepeated)
        {
            return string.Join(", ", ReadRepeated(message, jsonName));
        }
        if (field.HasPresence && !field.Accessor.HasValue(message))
        {
            return null;
        }

        var raw = field.Accessor.GetValue(message);
        if (field.FieldType is FieldType.Message or FieldType.Group)
        {
            // A nested message renders as its own JSON object, which no expectation
            // in this corpus names by field, so the type name is the useful answer.
            return (raw as IMessage)?.Descriptor.Name;
        }
        return field.FieldType switch
        {
            FieldType.String => raw as string,
            FieldType.Bool => raw is true ? "true" : "false",
            FieldType.Enum => EnumName(field, raw),
            FieldType.UInt64 or FieldType.Fixed64 =>
                Convert.ToUInt64(raw, System.Globalization.CultureInfo.InvariantCulture)
                    .ToString(System.Globalization.CultureInfo.InvariantCulture),
            FieldType.Int64 or FieldType.SFixed64 or FieldType.SInt64 =>
                Convert.ToInt64(raw, System.Globalization.CultureInfo.InvariantCulture)
                    .ToString(System.Globalization.CultureInfo.InvariantCulture),
            _ => raw?.ToString(),
        };
    }

    /// <summary>A repeated field's values, rendered as the recording renders them.</summary>
    private static IReadOnlyList<string> ReadRepeated(IMessage message, string jsonName)
    {
        var field = Find(message.Descriptor, jsonName);
        if (field is null)
        {
            return [];
        }
        var values = new List<string>();
        foreach (var item in (System.Collections.IEnumerable)field.Accessor.GetValue(message)!)
        {
            values.Add(field.FieldType == FieldType.Enum
                ? EnumName(field, item)
                : item?.ToString() ?? string.Empty);
        }
        return values;
    }

    /// <summary>
    /// An enum field's value as the name the recording spells it.
    /// </summary>
    /// <remarks>
    /// Through the descriptor, and never by casting the value: a generated C#
    /// message boxes its own CLR enum (<c>ApprovalState.Approved</c>) rather than an
    /// <see cref="EnumValueDescriptor"/>, so the cast throws
    /// <see cref="InvalidCastException"/>. <c>CompactJson</c> documents the same trap
    /// on the write side.
    ///
    /// A number the descriptor does not declare — an enum value from a newer server
    /// than this SDK was generated from — is rendered as its decimal text, which is
    /// what proto3 JSON permits and what keeps an unknown value from reading as a
    /// missing field.
    /// </remarks>
    private static string EnumName(FieldDescriptor field, object? value)
    {
        var number = value is EnumValueDescriptor descriptor
            ? descriptor.Number
            : value is null ? 0 : Convert.ToInt32(value, System.Globalization.CultureInfo.InvariantCulture);
        return field.EnumType?.FindValueByNumber(number)?.Name
               ?? number.ToString(System.Globalization.CultureInfo.InvariantCulture);
    }

    private static FieldDescriptor? Find(MessageDescriptor descriptor, string jsonName) =>
        descriptor.FindFieldByName(ToSnakeCase(jsonName)) ?? descriptor.FindFieldByName(jsonName);

    /// <summary>
    /// The proto name for a proto3 JSON name: <c>apiVersions</c> is
    /// <c>api_versions</c>.
    /// </summary>
    /// <remarks>
    /// Written out rather than delegated to the descriptor's own <c>JsonName</c>,
    /// because that is the answer for a name the proto declares in this shape — a
    /// corpus that spelled one in a shape the proto does not use is a corpus bug,
    /// and falling back to the JSON name on the descriptor above covers the
    /// remaining case without a guess.
    /// </remarks>
    internal static string ToSnakeCase(string jsonName)
    {
        var builder = new StringBuilder(jsonName.Length + 4);
        for (var index = 0; index < jsonName.Length; index++)
        {
            var character = jsonName[index];
            if (char.IsUpper(character) && index > 0)
            {
                builder.Append('_');
            }
            builder.Append(char.ToLowerInvariant(character));
        }
        return builder.ToString();
    }
}