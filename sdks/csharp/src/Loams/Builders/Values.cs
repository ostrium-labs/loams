// Plain .NET values as `google.protobuf.Value` and `Struct`, for document
// sources, filter terms and SQL parameters (SDKP §2.3, §2.4).

using System.Collections;
using System.Globalization;
using Google.Protobuf.WellKnownTypes;
using Loams.Collection.V1;

namespace Loams;

/// <summary>Converts plain .NET values to and from protobuf <c>Value</c> and <c>Struct</c>.</summary>
public static class Values
{
    /// <summary>
    /// A value: null, bool, any number, string, a <see cref="Value"/> or
    /// <see cref="Struct"/>, a dictionary with string keys, or any other sequence
    /// (as a list). A <c>float[]</c> is a list of numbers.
    /// </summary>
    public static Value Of(object? value) => value switch
    {
        null => Value.ForNull(),
        Value v => v,
        Struct s => Value.ForStruct(s),
        ListValue l => new Value { ListValue = l },
        string s => Value.ForString(s),
        bool b => Value.ForBool(b),
        double d => Value.ForNumber(d),
        float f => Value.ForNumber(f),
        decimal m => Value.ForNumber((double)m),
        int or long or short or sbyte or byte or uint or ushort => Value.ForNumber(Convert.ToDouble(value, CultureInfo.InvariantCulture)),
        // A 64-bit unsigned id above 2^53 does not survive a double; proto3 JSON
        // and the id parsers take it as a decimal string.
        ulong u => u <= (1UL << 53) ? Value.ForNumber(u) : Value.ForString(u.ToString(CultureInfo.InvariantCulture)),
        DocumentId id => DocumentIdValue(id),
        DateTime t => Value.ForString(t.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss.fffffff'Z'", CultureInfo.InvariantCulture)),
        DateTimeOffset t => Value.ForString(t.UtcDateTime.ToString("yyyy-MM-dd'T'HH:mm:ss.fffffff'Z'", CultureInfo.InvariantCulture)),
        Guid g => Value.ForString(g.ToString()),
        IDictionary<string, object?> map => Value.ForStruct(StructOf(map)),
        IDictionary map => Value.ForStruct(StructOf(map)),
        IEnumerable sequence => new Value { ListValue = ListOf(sequence) },
        _ => throw new ArgumentException(
            $"cannot send a {value.GetType().Name} as a protobuf Value: pass a bool, number, string, " +
            "dictionary, sequence, Value or Struct", nameof(value)),
    };

    /// <summary>A <see cref="Struct"/> from a dictionary with string keys.</summary>
    public static Struct StructOf(IEnumerable<KeyValuePair<string, object?>> map)
    {
        ArgumentNullException.ThrowIfNull(map);
        var result = new Struct();
        foreach (var (key, value) in map)
        {
            result.Fields[key] = Of(value);
        }
        return result;
    }

    private static Struct StructOf(IDictionary map)
    {
        var result = new Struct();
        foreach (DictionaryEntry entry in map)
        {
            result.Fields[entry.Key as string ?? throw new ArgumentException("a Struct's keys are strings")] = Of(entry.Value);
        }
        return result;
    }

    /// <summary>A <see cref="Struct"/> from an object's public properties, or a dictionary.</summary>
    public static Struct StructFrom(object source) => source switch
    {
        null => throw new ArgumentNullException(nameof(source)),
        Struct s => s,
        IEnumerable<KeyValuePair<string, object?>> map => StructOf(map),
        IDictionary map => StructOf(map),
        _ => StructOf(source.GetType().GetProperties()
            .Where((p) => p.CanRead && p.GetIndexParameters().Length == 0)
            .Select((p) => new KeyValuePair<string, object?>(p.Name, p.GetValue(source)))),
    };

    /// <summary>A <see cref="ListValue"/> from a sequence.</summary>
    public static ListValue ListOf(IEnumerable sequence)
    {
        ArgumentNullException.ThrowIfNull(sequence);
        var list = new ListValue();
        foreach (var item in sequence)
        {
            list.Values.Add(Of(item));
        }
        return list;
    }

    /// <summary>A <see cref="Value"/> as a plain .NET value: null, bool, double, string, list or dictionary.</summary>
    public static object? ToPlain(Value? value) => value?.KindCase switch
    {
        null or Value.KindOneofCase.None or Value.KindOneofCase.NullValue => null,
        Value.KindOneofCase.BoolValue => value.BoolValue,
        Value.KindOneofCase.NumberValue => value.NumberValue,
        Value.KindOneofCase.StringValue => value.StringValue,
        Value.KindOneofCase.ListValue => value.ListValue.Values.Select(ToPlain).ToList(),
        Value.KindOneofCase.StructValue => ToPlain(value.StructValue),
        _ => null,
    };

    /// <summary>A <see cref="Struct"/> as a dictionary of plain values.</summary>
    public static Dictionary<string, object?> ToPlain(Struct? value) =>
        value?.Fields.ToDictionary((kv) => kv.Key, (kv) => ToPlain(kv.Value), StringComparer.Ordinal)
        ?? new Dictionary<string, object?>(StringComparer.Ordinal);

    // `Query.ids` carries the `DocumentId` shape as a Value: `{"uint": "1"}`
    // (exact past 2^53, and the builder corpus's spelling), a string, or
    // `{"uuid": "…"}`.
    internal static Value DocumentIdValue(DocumentId id) => id.IdCase switch
    {
        DocumentId.IdOneofCase.Uint =>
            Value.ForStruct(new Struct { Fields = { ["uint"] = Value.ForString(id.Uint.ToString(CultureInfo.InvariantCulture)) } }),
        DocumentId.IdOneofCase.String => Value.ForString(id.String),
        DocumentId.IdOneofCase.Uuid => Value.ForStruct(new Struct { Fields = { ["uuid"] = Value.ForString(id.Uuid) } }),
        _ => throw new ArgumentException("a DocumentId must be set", nameof(id)),
    };
}
