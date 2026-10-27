package dev.loams;

import com.google.protobuf.ListValue;
import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * The schema builder (SDKP §2.6): a collection's schema as the {@code Struct} {@code
 * CreateCollectionRequest.schema} takes, in the shape the engine's schema parser reads ({@code
 * crates/loams-query/src/json/schema.rs}).
 *
 * <pre>{@code
 * Struct schema = new Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4).build();
 * }</pre>
 *
 * <p>Each field takes {@code indexed}, {@code fast} and {@code sourcePath} overrides through the
 * {@link Field} form: {@code schema.field(Schema.Field.keyword("tags").fast(false))}.
 */
public final class Schema {

    /** One field's settings. */
    public static final class Field {
        private final String name;
        private final Value kind;
        private boolean indexed = true;
        private boolean fast;
        private String sourcePath;

        private Field(String name, Value kind, boolean fast) {
            this.name = name;
            this.kind = kind;
            this.fast = fast;
        }

        public static Field text(String name, String analyzer, boolean positions) {
            Struct text = Struct.newBuilder()
                    .putFields("analyzer", str(analyzer))
                    .putFields("positions", Value.newBuilder().setBoolValue(positions).build())
                    .build();
            Value kind = Value.newBuilder()
                    .setStructValue(Struct.newBuilder().putFields("text", Value.newBuilder().setStructValue(text).build()))
                    .build();
            return new Field(name, kind, false);
        }

        public static Field keyword(String name) {
            return new Field(name, str("keyword"), true);
        }

        public static Field of(String name, String kind, boolean fast) {
            return new Field(name, str(kind), fast);
        }

        public Field indexed(boolean value) {
            this.indexed = value;
            return this;
        }

        public Field fast(boolean value) {
            this.fast = value;
            return this;
        }

        public Field sourcePath(String value) {
            this.sourcePath = value;
            return this;
        }

        Value build() {
            return Value.newBuilder().setStructValue(Struct.newBuilder()
                            .putFields("name", str(name))
                            .putFields("source_path", str(sourcePath == null ? name : sourcePath))
                            .putFields("kind", kind)
                            .putFields("indexed", Value.newBuilder().setBoolValue(indexed).build())
                            .putFields("fast", Value.newBuilder().setBoolValue(fast).build()))
                    .build();
        }
    }

    private final List<Value> fields = new ArrayList<>();
    private final List<Value> vectors = new ArrayList<>();
    private final List<Value> sparse = new ArrayList<>();
    private final Map<String, String> annotations = new LinkedHashMap<>();
    private String dynamic = "ignore";
    private int maxFields = 1000;

    /** A full-text field with the standard analyzer and positions. */
    public Schema text(String name) {
        return field(Field.text(name, "standard", true));
    }

    /** A full-text field. */
    public Schema text(String name, String analyzer, boolean positions) {
        return field(Field.text(name, analyzer, positions));
    }

    /** An exact-match string field. */
    public Schema keyword(String name) {
        return field(Field.keyword(name));
    }

    /** A signed 64-bit integer field. */
    public Schema i64(String name) {
        return field(Field.of(name, "i64", true));
    }

    /** A 64-bit float field. */
    public Schema f64(String name) {
        return field(Field.of(name, "f64", true));
    }

    /** A boolean field. */
    public Schema bool(String name) {
        return field(Field.of(name, "bool", true));
    }

    /** A timestamp field (the engine's {@code date} kind). */
    public Schema timestamp(String name) {
        return field(Field.of(name, "date", true));
    }

    /** A UUID field. */
    public Schema uuid(String name) {
        return field(Field.of(name, "uuid", true));
    }

    /** A JSON field. */
    public Schema json(String name) {
        return field(Field.of(name, "json", false));
    }

    /** A field with overrides. */
    public Schema field(Field field) {
        fields.add(field.build());
        return this;
    }

    /** A dense vector of {@code dim} floats with cosine distance. */
    public Schema vector(String name, int dim) {
        return vector(name, dim, "cosine");
    }

    /** A dense vector; {@code distance} is cosine, dot, euclid or manhattan. */
    public Schema vector(String name, int dim, String distance) {
        vectors.add(Value.newBuilder().setStructValue(Struct.newBuilder()
                        .putFields("name", str(name))
                        .putFields("dim", Value.newBuilder().setNumberValue(dim).build())
                        .putFields("distance", str(distance)))
                .build());
        return this;
    }

    /** A sparse vector; {@code modifier} is null, {@code none} or {@code idf}. */
    public Schema sparseVector(String name, String modifier) {
        Struct.Builder spec = Struct.newBuilder().putFields("name", str(name));
        if (modifier != null) {
            spec.putFields("modifier", str(modifier));
        }
        sparse.add(Value.newBuilder().setStructValue(spec).build());
        return this;
    }

    /** What happens to a source key no field names: {@code ignore}, {@code strict} or {@code map}. */
    public Schema dynamic(String mode) {
        this.dynamic = mode;
        return this;
    }

    /** At most this many fields, vectors and sparse vectors together. */
    public Schema maxFields(int n) {
        this.maxFields = n;
        return this;
    }

    /** An annotation (keys {@code es.*}, {@code qdrant.*} or {@code loams.*}). */
    public Schema annotation(String key, String value) {
        annotations.put(key, value);
        return this;
    }

    /** The schema as the request's {@code Struct}. */
    public Struct build() {
        Struct.Builder schema = Struct.newBuilder()
                .putFields("fields", listOf(fields))
                .putFields("vectors", listOf(vectors))
                .putFields("sparse_vectors", listOf(sparse))
                .putFields("dynamic", str(dynamic))
                .putFields("max_fields", Value.newBuilder().setNumberValue(maxFields).build());
        if (!annotations.isEmpty()) {
            schema.putFields("annotations", Values.of(annotations));
        }
        return schema.build();
    }

    private static Value listOf(List<Value> values) {
        return Value.newBuilder().setListValue(ListValue.newBuilder().addAllValues(values)).build();
    }

    private static Value str(String s) {
        return Value.newBuilder().setStringValue(s).build();
    }
}
