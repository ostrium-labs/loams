// The proto3 JSON mapping, written out (R10).
//
// `Google.Protobuf` ships a JSON formatter, and it is not what this SDK sends.
// The formatter emits `{ "field": value }` — a space after the opening brace and
// around every colon and comma — while the proto3 JSON mapping as the corpus
// records it is compact: `{"approvalId":"apr_…","revision":"1","decision":…}`.
// `sdks/conformance/fixture-server.mjs` compares the request bytes it receives
// against the recording **byte for byte** and answers 400 on a difference, so a
// formatter's spaces make this SDK fail every JSON fixture for a reason that has
// nothing to do with conformance. Hence this file.
//
// # What "the proto3 JSON mapping" means here, precisely
//
// Four rules, and each one is a place the mapping is observable in the corpus:
//
//   - **compact** — no whitespace between tokens at all;
//   - **field order by field number**, which is what a serializer writing a
//     descriptor walk produces and what the recordings have;
//   - **defaults omitted** — proto3's implicit presence, so an empty
//     `GetInstanceRequest` is `{}` and not `{"name":""}`;
//   - **64-bit integers as strings** — `"revision":"1"`, not `"revision":1`,
//     because a JSON number cannot hold the full range of an `int64`. The corpus
//     depends on this: `mock_state_idempotent_decide`'s expectation is on
//     `revision: "2"`, a string.
//
// Enums are written by **name**, never by number, because the recordings name
// them (`"DECISION_KIND_APPROVE"`, `"APPROVAL_STATE_PENDING"`) and a server
// rejects a number where the proto3 JSON mapping requires a name.
//
// # What is deliberately not here
//
// **Well-known types.** `Timestamp`, `Duration`, `Any`, `Struct`, `Value`,
// `ListValue` and the field masks each have their own JSON form, which the
// generated formatter implements and this does not. The requests this SDK sends
// today carry none of them — `GetInstance`, `DecideApproval`,
// `WatchApprovals`, `Query`, `Mutate`, `Deploy` — so the gap cannot be reached
// from the facade. Rather than write a half-correct mapping for types no call
// sends, `Format` **refuses** a message containing one, naming the type. A
// request that cannot be encoded is a failure at the boundary with a message
// saying so, which is better than a body the server will reject with a parse
// error three layers away. Responses go the other way and use
// `Google.Protobuf`'s parser, which does understand them.
//
// **Extensions and unknown fields.** Neither appears in a request this SDK
// builds, and writing them would need a registry that has nothing to register.

using System.Globalization;
using System.Text;
using Google.Protobuf;
using Google.Protobuf.Reflection;

namespace Loams;

/// <summary>
/// Writes the proto3 JSON mapping compactly, in field-number order, with proto3
/// defaults omitted. See the file comment for why this exists rather than
/// <c>Google.Protobuf</c>'s formatter.
/// </summary>
public static class CompactJson
{
    /// <summary>The message's proto3 JSON form.</summary>
    /// <exception cref="NotSupportedException">
    /// The message contains a well-known type, whose JSON mapping is not
    /// implemented here. Named, rather than silently mis-encoded.
    /// </exception>
    public static string Format(IMessage message)
    {
        ArgumentNullException.ThrowIfNull(message);
        var builder = new StringBuilder();
        WriteMessage(builder, message);
        return builder.ToString();
    }

    private static void WriteMessage(StringBuilder builder, IMessage message)
    {
        var descriptor = message.Descriptor;
        if (IsWellKnown(descriptor.FullName))
        {
            throw new NotSupportedException(
                $"{descriptor.FullName} has its own proto3 JSON mapping, which this writer does not " +
                "implement; no Loams facade call sends one, and a body the server cannot parse is worse " +
                "than a refusal at the boundary");
        }

        builder.Append('{');
        var first = true;
        // `InFieldNumberOrder` is the ordering rule, and it is a property of the
        // descriptor rather than of the dictionary the generated class happens to
        // expose.
        foreach (var field in descriptor.Fields.InFieldNumberOrder())
        {
            if (!ShouldWrite(message, field))
            {
                continue;
            }
            if (!first)
            {
                builder.Append(',');
            }
            first = false;
            WriteString(builder, field.JsonName);
            builder.Append(':');
            WriteField(builder, field, field.Accessor.GetValue(message));
        }
        builder.Append('}');
    }

    /// <summary>
    /// Whether a field is written: proto3 implicit presence means a field at its
    /// default is absent, and a field with explicit presence (a proto3
    /// <c>optional</c>, a message field, a oneof case) is written when it is set.
    /// </summary>
    private static bool ShouldWrite(IMessage message, FieldDescriptor field)
    {
        if (field.ContainingOneof is not null)
        {
            // A oneof case is written when it is the set one, even at its default
            // value: the case *is* the information.
            return field.Accessor.HasValue(message);
        }
        if (field.IsMap || field.IsRepeated)
        {
            return field.Accessor.HasValue(message);
        }
        if (field.HasPresence)
        {
            return field.Accessor.HasValue(message);
        }
        if (field.FieldType is FieldType.Message or FieldType.Group)
        {
            // A message field has presence even when proto3 does not say so:
            // unset is null and set is an instance.
            return field.Accessor.GetValue(message) is not null;
        }
        return !IsDefaultValue(field.FieldType, field.Accessor.GetValue(message));
    }

    private static bool IsDefaultValue(FieldType type, object? value) => type switch
    {
        FieldType.Bool => value is false or null,
        FieldType.String => string.IsNullOrEmpty(value as string),
        FieldType.Bytes => (value as ByteString)?.Length is 0 or null,
        // The CLR value of a generated message's enum field is the **generated
        // enum**, not an `EnumValueDescriptor`: `FieldAccessor.GetValue` boxes
        // `DecisionKind.Approve`. Casting to the descriptor therefore yields
        // `null`, and reading that as "the default" dropped every set enum field
        // from the request. `EnumNumber` accepts both shapes so the answer does
        // not depend on which kind of descriptor is behind the message.
        FieldType.Enum => EnumNumber(value) is not { } number || number == 0,
        FieldType.Int32 or FieldType.SInt32 or FieldType.SFixed32 => Convert.ToInt32(value, CultureInfo.InvariantCulture) == 0,
        FieldType.UInt32 or FieldType.Fixed32 => Convert.ToUInt32(value, CultureInfo.InvariantCulture) == 0,
        FieldType.Int64 or FieldType.SInt64 or FieldType.SFixed64 => Convert.ToInt64(value, CultureInfo.InvariantCulture) == 0,
        FieldType.UInt64 or FieldType.Fixed64 => Convert.ToUInt64(value, CultureInfo.InvariantCulture) == 0,
        FieldType.Float => Convert.ToSingle(value, CultureInfo.InvariantCulture) == 0f,
        FieldType.Double => Convert.ToDouble(value, CultureInfo.InvariantCulture) == 0d,
        _ => value is null,
    };

    private static void WriteField(StringBuilder builder, FieldDescriptor field, object? value)
    {
        if (field.IsMap)
        {
            WriteMap(builder, field, value);
            return;
        }
        if (field.IsRepeated)
        {
            builder.Append('[');
            var first = true;
            foreach (var item in (System.Collections.IEnumerable)value!)
            {
                if (!first)
                {
                    builder.Append(',');
                }
                first = false;
                WriteScalar(builder, field, item);
            }
            builder.Append(']');
            return;
        }
        WriteScalar(builder, field, value);
    }

    private static void WriteMap(StringBuilder builder, FieldDescriptor field, object? value)
    {
        builder.Append('{');
        var first = true;
        // A map's key is a string in proto3 JSON, whatever its declared type, and
        // `Int32` keys are written as their decimal text.
        foreach (System.Collections.DictionaryEntry entry in (System.Collections.IDictionary)value!)
        {
            if (!first)
            {
                builder.Append(',');
            }
            first = false;
            WriteString(builder, Convert.ToString(entry.Key, CultureInfo.InvariantCulture)!);
            builder.Append(':');
            WriteScalar(builder, MapValueOf(field), entry.Value);
        }
        builder.Append('}');
    }

    /// <summary>
    /// A map field's value descriptor.
    /// </summary>
    /// <remarks>
    /// A map is not a field type: the proto compiler synthesises a nested
    /// <c>map&lt;K, V&gt;</c> *message* whose field 1 is the key and whose field 2 is
    /// the value, and <see cref="FieldDescriptor"/> has no accessor for the value
    /// type. It is read off that synthetic message, which is where protoc put it.
    /// </remarks>
    private static FieldDescriptor MapValueOf(FieldDescriptor mapField) =>
        mapField.MessageType.FindFieldByName("value")
        ?? throw new InvalidOperationException(
            $"{mapField.FullName} is a map with no value field, which no proto compiler emits");

    private static void WriteScalar(StringBuilder builder, FieldDescriptor field, object? value)
    {
        switch (field.FieldType)
        {
            case FieldType.Message:
            case FieldType.Group:
                WriteMessage(builder, (IMessage)value!);
                return;

            case FieldType.Enum:
                // By name, never by number: the mapping requires it and a server
                // rejects a number.
                //
                // The name is resolved through the descriptor's enum type rather
                // than cast off the value, because the value is the generated CLR
                // enum (see `EnumNumber`). A number the enum type does not
                // declare — a value from a newer server — is written as its
                // decimal text, which is what the proto3 JSON mapping permits for
                // an unknown enum value and is the only thing that keeps such a
                // field from being dropped.
                WriteString(builder, EnumName(field, value));
                return;

            case FieldType.Bool:
                builder.Append((bool)value! ? "true" : "false");
                return;

            case FieldType.String:
                WriteString(builder, (string)value!);
                return;

            case FieldType.Bytes:
                WriteString(builder, Convert.ToBase64String(((ByteString)value!).ToByteArray()));
                return;

            // 64-bit integers are JSON **strings**: a JSON number is a double to
            // most readers and cannot hold an int64's range. The corpus's
            // `revision` expectations are strings for exactly this reason.
            case FieldType.Int64:
            case FieldType.SInt64:
            case FieldType.SFixed64:
                WriteString(builder, Convert.ToInt64(value, CultureInfo.InvariantCulture)
                    .ToString(CultureInfo.InvariantCulture));
                return;

            case FieldType.UInt64:
            case FieldType.Fixed64:
                WriteString(builder, Convert.ToUInt64(value, CultureInfo.InvariantCulture)
                    .ToString(CultureInfo.InvariantCulture));
                return;

            case FieldType.Int32:
            case FieldType.SInt32:
            case FieldType.SFixed32:
                builder.Append(Convert.ToInt32(value, CultureInfo.InvariantCulture)
                    .ToString(CultureInfo.InvariantCulture));
                return;

            case FieldType.UInt32:
            case FieldType.Fixed32:
                builder.Append(Convert.ToUInt32(value, CultureInfo.InvariantCulture)
                    .ToString(CultureInfo.InvariantCulture));
                return;

            case FieldType.Float:
                builder.Append(FormatDouble(Convert.ToSingle(value, CultureInfo.InvariantCulture)));
                return;

            case FieldType.Double:
                builder.Append(FormatDouble(Convert.ToDouble(value, CultureInfo.InvariantCulture)));
                return;

            default:
                throw new NotSupportedException(
                    $"field {field.FullName} is a {field.FieldType}, which this writer does not implement");
        }
    }

    /// <summary>
    /// A JSON number for a float or a double: <c>InvariantCulture</c> always, and
    /// a decimal point rather than a comma. A locale-sensitive format here would
    /// make the same request bytes differ per machine, which is the class of bug
    /// a recorded corpus exists to catch.
    /// </summary>
    private static string FormatDouble(double value)
    {
        if (double.IsNaN(value) || double.IsInfinity(value))
        {
            throw new NotSupportedException(
                "a NaN or infinite double has no proto3 JSON form and cannot be serialized");
        }
        return value.ToString("R", CultureInfo.InvariantCulture);
    }

    /// <summary>
    /// Writes a JSON string, escaping exactly what RFC 8259 requires. It is
    /// written out rather than delegated to <c>JsonEncodedText</c> so this file
    /// has no dependency on the serializer's escape policy changing under it —
    /// a body the corpus has never seen is a 400.
    /// </summary>
    private static void WriteString(StringBuilder builder, string value)
    {
        builder.Append('"');
        foreach (var character in value)
        {
            switch (character)
            {
                case '"': builder.Append("\\\""); break;
                case '\\': builder.Append("\\\\"); break;
                case '\b': builder.Append("\\b"); break;
                case '\f': builder.Append("\\f"); break;
                case '\n': builder.Append("\\n"); break;
                case '\r': builder.Append("\\r"); break;
                case '\t': builder.Append("\\t"); break;
                default:
                    if (character < 0x20)
                    {
                        builder.Append("\\u").Append(((int)character).ToString("x4", CultureInfo.InvariantCulture));
                    }
                    else
                    {
                        builder.Append(character);
                    }
                    break;
            }
        }
        builder.Append('"');
    }

    /// The number of an enum field's value, whichever shape it arrives in.
    ///
    /// `FieldAccessor.GetValue` hands back the **generated CLR enum** —
    /// `DecisionKind.Approve` — not an `EnumValueDescriptor`, so casting the
    /// value to a descriptor yields `null` and reading that as "the default"
    /// drops every set enum field from the request. Both shapes are accepted so
    /// the answer does not depend on which one is behind the message; the
    /// descriptor path is kept because it is the one protobuf's own
    /// reflection-based readers take.
    ///
    /// `null` rather than 0 for "no value at all", because 0 is a legal enum
    /// number and proto3 says the first value is the default — the two must not
    /// be confused when deciding whether to omit the field.
    private static int? EnumNumber(object? value) => value switch
    {
        null => null,
        EnumValueDescriptor descriptor => descriptor.Number,
        Enum => Convert.ToInt32(value, CultureInfo.InvariantCulture),
        _ => Convert.ToInt32(value, CultureInfo.InvariantCulture),
    };

    /// The proto3 JSON name for an enum field's value.
    ///
    /// Resolved through the descriptor's enum type rather than cast off the
    /// value, for the reason `EnumNumber` gives. A number the enum type does
    /// not declare — a value from a newer server than this SDK was generated
    /// from — has no name, and the proto3 JSON mapping permits its decimal text.
    /// Writing the number rather than omitting the field is what keeps such a
    /// field from being silently dropped, which would turn a forward-compatible
    /// server response into a request that changes its meaning.
    private static string EnumName(FieldDescriptor field, object? value)
    {
        int number = EnumNumber(value) ?? 0;
        EnumValueDescriptor? named = field.EnumType?.FindValueByNumber(number);
        return named?.Name ?? number.ToString(CultureInfo.InvariantCulture);
    }

    private static bool IsWellKnown(string fullName) => fullName switch
    {
        "google.protobuf.Timestamp" or
        "google.protobuf.Duration" or
        "google.protobuf.Any" or
        "google.protobuf.Struct" or
        "google.protobuf.Value" or
        "google.protobuf.ListValue" or
        "google.protobuf.FieldMask" or
        "google.protobuf.Empty" => true,
        _ => false,
    };
}