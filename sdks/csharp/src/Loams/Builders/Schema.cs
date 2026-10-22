// The schema builder (SDKP §2.6): a collection's schema as the `Struct`
// `CreateCollectionRequest.schema` takes, in the shape the engine's schema parser
// reads (`crates/loams-query/src/json/schema.rs`).
//
//     var schema = new Schema()
//         .Text("title").Text("body").Keyword("tags").I64("n")
//         .Vector("embedding", 4);

using Google.Protobuf.WellKnownTypes;

namespace Loams;

/// <summary>Builds a collection schema.</summary>
public sealed class Schema
{
    private readonly List<Value> _fields = [];
    private readonly List<Value> _vectors = [];
    private readonly List<Value> _sparse = [];
    private readonly Dictionary<string, string> _annotations = new(StringComparer.Ordinal);
    private string _dynamic = "ignore";
    private uint _maxFields = 1000;

    /// <summary>A full-text field.</summary>
    public Schema Text(string name, string analyzer = "standard", bool positions = true,
        bool indexed = true, bool fast = false, string? sourcePath = null)
    {
        var kind = new Struct
        {
            Fields =
            {
                ["text"] = Value.ForStruct(new Struct
                {
                    Fields = { ["analyzer"] = Value.ForString(analyzer), ["positions"] = Value.ForBool(positions) },
                }),
            },
        };
        return Field(name, Value.ForStruct(kind), indexed, fast, sourcePath);
    }

    /// <summary>An exact-match string field.</summary>
    public Schema Keyword(string name, bool indexed = true, bool fast = true, string? sourcePath = null) =>
        Field(name, Value.ForString("keyword"), indexed, fast, sourcePath);

    /// <summary>A signed 64-bit integer field.</summary>
    public Schema I64(string name, bool indexed = true, bool fast = true, string? sourcePath = null) =>
        Field(name, Value.ForString("i64"), indexed, fast, sourcePath);

    /// <summary>A 64-bit float field.</summary>
    public Schema F64(string name, bool indexed = true, bool fast = true, string? sourcePath = null) =>
        Field(name, Value.ForString("f64"), indexed, fast, sourcePath);

    /// <summary>A boolean field.</summary>
    public Schema Bool(string name, bool indexed = true, bool fast = true, string? sourcePath = null) =>
        Field(name, Value.ForString("bool"), indexed, fast, sourcePath);

    /// <summary>A timestamp field (the engine's <c>date</c> kind).</summary>
    public Schema Timestamp(string name, bool indexed = true, bool fast = true, string? sourcePath = null) =>
        Field(name, Value.ForString("date"), indexed, fast, sourcePath);

    /// <summary>A UUID field.</summary>
    public Schema Uuid(string name, bool indexed = true, bool fast = true, string? sourcePath = null) =>
        Field(name, Value.ForString("uuid"), indexed, fast, sourcePath);

    /// <summary>A JSON field.</summary>
    public Schema Json(string name, bool indexed = true, bool fast = false, string? sourcePath = null) =>
        Field(name, Value.ForString("json"), indexed, fast, sourcePath);

    /// <summary>A dense vector of <paramref name="dim"/> floats.</summary>
    public Schema Vector(string name, uint dim, string distance = "cosine")
    {
        _vectors.Add(Value.ForStruct(new Struct
        {
            Fields =
            {
                ["name"] = Value.ForString(name),
                ["dim"] = Value.ForNumber(dim),
                ["distance"] = Value.ForString(distance),
            },
        }));
        return this;
    }

    /// <summary>A sparse vector; <paramref name="modifier"/> is <c>none</c> or <c>idf</c>.</summary>
    public Schema SparseVector(string name, string? modifier = null)
    {
        var spec = new Struct { Fields = { ["name"] = Value.ForString(name) } };
        if (modifier is not null)
        {
            spec.Fields["modifier"] = Value.ForString(modifier);
        }
        _sparse.Add(Value.ForStruct(spec));
        return this;
    }

    /// <summary>What happens to a source key no field names: <c>ignore</c>, <c>strict</c> or <c>map</c>.</summary>
    public Schema Dynamic(string mode)
    {
        _dynamic = mode;
        return this;
    }

    /// <summary>At most this many fields, vectors and sparse vectors together.</summary>
    public Schema MaxFields(uint n)
    {
        _maxFields = n;
        return this;
    }

    /// <summary>An annotation (keys <c>es.*</c>, <c>qdrant.*</c> or <c>loams.*</c>).</summary>
    public Schema Annotation(string key, string value)
    {
        _annotations[key] = value;
        return this;
    }

    private Schema Field(string name, Value kind, bool indexed, bool fast, string? sourcePath)
    {
        _fields.Add(Value.ForStruct(new Struct
        {
            Fields =
            {
                ["name"] = Value.ForString(name),
                ["source_path"] = Value.ForString(sourcePath ?? name),
                ["kind"] = kind,
                ["indexed"] = Value.ForBool(indexed),
                ["fast"] = Value.ForBool(fast),
            },
        }));
        return this;
    }

    /// <summary>The schema as the request's <c>Struct</c>.</summary>
    public Struct Build()
    {
        var schema = new Struct
        {
            Fields =
            {
                ["fields"] = Value.ForList([.. _fields]),
                ["vectors"] = Value.ForList([.. _vectors]),
                ["sparse_vectors"] = Value.ForList([.. _sparse]),
                ["dynamic"] = Value.ForString(_dynamic),
                ["max_fields"] = Value.ForNumber(_maxFields),
            },
        };
        if (_annotations.Count > 0)
        {
            schema.Fields["annotations"] = Value.ForStruct(Values.StructOf(
                _annotations.Select((kv) => new KeyValuePair<string, object?>(kv.Key, kv.Value))));
        }
        return schema;
    }

    public static implicit operator Struct(Schema schema) => schema.Build();
}
