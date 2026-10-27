package dev.loams;

import com.google.protobuf.ListValue;
import com.google.protobuf.NullValue;
import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import dev.loams.gen.loams.collection.v1.DocumentId;
import java.lang.reflect.RecordComponent;
import java.util.ArrayList;
import java.util.Collection;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;

/**
 * Plain Java values as {@code google.protobuf.Value} and {@code Struct}, for document sources,
 * filter terms and SQL parameters (SDKP §2.3, §2.4), and document ids.
 */
public final class Values {

    private Values() {}

    /**
     * A value: null, Boolean, any Number, CharSequence, a {@link Value} or {@link Struct}, a Map with
     * string keys, a Collection or array (as a list), a record (its components), or a {@link
     * DocumentId}.
     */
    public static Value of(Object value) {
        if (value == null) {
            return Value.newBuilder().setNullValue(NullValue.NULL_VALUE).build();
        }
        if (value instanceof Value v) {
            return v;
        }
        if (value instanceof Struct s) {
            return Value.newBuilder().setStructValue(s).build();
        }
        if (value instanceof ListValue l) {
            return Value.newBuilder().setListValue(l).build();
        }
        if (value instanceof CharSequence text) {
            return Value.newBuilder().setStringValue(text.toString()).build();
        }
        if (value instanceof Boolean b) {
            return Value.newBuilder().setBoolValue(b).build();
        }
        if (value instanceof Number n) {
            return Value.newBuilder().setNumberValue(n.doubleValue()).build();
        }
        if (value instanceof UUID uuid) {
            return Value.newBuilder().setStringValue(uuid.toString()).build();
        }
        if (value instanceof DocumentId id) {
            return documentIdValue(id);
        }
        if (value instanceof Map<?, ?> map) {
            return Value.newBuilder().setStructValue(struct(map)).build();
        }
        if (value instanceof Iterable<?> items) {
            return Value.newBuilder().setListValue(list(items)).build();
        }
        if (value instanceof float[] floats) {
            ListValue.Builder list = ListValue.newBuilder();
            for (float f : floats) {
                list.addValues(Value.newBuilder().setNumberValue(f));
            }
            return Value.newBuilder().setListValue(list).build();
        }
        if (value instanceof double[] doubles) {
            ListValue.Builder list = ListValue.newBuilder();
            for (double d : doubles) {
                list.addValues(Value.newBuilder().setNumberValue(d));
            }
            return Value.newBuilder().setListValue(list).build();
        }
        if (value instanceof Object[] array) {
            return Value.newBuilder().setListValue(list(List.of(array))).build();
        }
        if (value instanceof Record record) {
            return Value.newBuilder().setStructValue(structOf(record)).build();
        }
        throw new IllegalArgumentException(
                "loams: cannot send a " + value.getClass().getSimpleName() + " as a protobuf Value");
    }

    /** A {@link Struct} from a Map with string keys. */
    public static Struct struct(Map<?, ?> map) {
        Struct.Builder out = Struct.newBuilder();
        for (Map.Entry<?, ?> entry : map.entrySet()) {
            if (!(entry.getKey() instanceof String key)) {
                throw new IllegalArgumentException("loams: a Struct's keys are strings");
            }
            out.putFields(key, of(entry.getValue()));
        }
        return out.build();
    }

    /** A {@link Struct} from key-value pairs: {@code struct("title", "a", "n", 1)}. */
    public static Struct struct(Object... pairs) {
        if (pairs.length % 2 != 0) {
            throw new IllegalArgumentException("loams: struct(...) takes key, value pairs");
        }
        Map<Object, Object> map = new LinkedHashMap<>();
        for (int i = 0; i < pairs.length; i += 2) {
            map.put(pairs[i], pairs[i + 1]);
        }
        return struct(map);
    }

    /** A {@link Struct} from a Struct, a Map, or a record's components. */
    public static Struct structFrom(Object source) {
        if (source instanceof Struct s) {
            return s;
        }
        if (source instanceof Map<?, ?> map) {
            return struct(map);
        }
        if (source instanceof Record record) {
            return structOf(record);
        }
        throw new IllegalArgumentException(
                "loams: a document source is a Struct, a Map or a record, not a "
                        + (source == null ? "null" : source.getClass().getSimpleName()));
    }

    private static Struct structOf(Record record) {
        Map<String, Object> map = new LinkedHashMap<>();
        for (RecordComponent component : record.getClass().getRecordComponents()) {
            try {
                map.put(component.getName(), component.getAccessor().invoke(record));
            } catch (ReflectiveOperationException e) {
                throw new IllegalArgumentException("loams: cannot read " + component.getName(), e);
            }
        }
        return struct(map);
    }

    /** A {@link ListValue} from any items. */
    public static ListValue list(Iterable<?> items) {
        ListValue.Builder out = ListValue.newBuilder();
        for (Object item : items) {
            out.addValues(of(item));
        }
        return out.build();
    }

    /** A {@link Value} as a plain Java value: null, Boolean, Double, String, List or Map. */
    public static Object toPlain(Value value) {
        if (value == null) {
            return null;
        }
        return switch (value.getKindCase()) {
            case BOOL_VALUE -> value.getBoolValue();
            case NUMBER_VALUE -> value.getNumberValue();
            case STRING_VALUE -> value.getStringValue();
            case LIST_VALUE -> {
                List<Object> out = new ArrayList<>();
                for (Value item : value.getListValue().getValuesList()) {
                    out.add(toPlain(item));
                }
                yield out;
            }
            case STRUCT_VALUE -> toPlain(value.getStructValue());
            default -> null;
        };
    }

    /** A {@link Struct} as a Map of plain values. */
    public static Map<String, Object> toPlain(Struct struct) {
        Map<String, Object> out = new LinkedHashMap<>();
        if (struct != null) {
            struct.getFieldsMap().forEach((k, v) -> out.put(k, toPlain(v)));
        }
        return out;
    }

    // ---- document ids ----

    /** A numeric document id (unsigned 64-bit). */
    public static DocumentId id(long id) {
        return DocumentId.newBuilder().setUint(id).build();
    }

    /** A string document id. */
    public static DocumentId id(String id) {
        return DocumentId.newBuilder().setString(id).build();
    }

    /** A UUID document id. */
    public static DocumentId id(UUID id) {
        return DocumentId.newBuilder().setUuid(id.toString()).build();
    }

    /** Document ids from longs, strings, UUIDs or DocumentIds. */
    public static List<DocumentId> ids(Object... ids) {
        List<DocumentId> out = new ArrayList<>();
        for (Object id : ids) {
            out.add(toId(id));
        }
        return out;
    }

    /** One document id from a long, int, String, UUID or DocumentId. */
    public static DocumentId toId(Object id) {
        if (id instanceof DocumentId d) {
            return d;
        }
        if (id instanceof Long l) {
            return id(l);
        }
        if (id instanceof Integer i) {
            if (i < 0) {
                throw new IllegalArgumentException("loams: a numeric document id is unsigned");
            }
            return id((long) i);
        }
        if (id instanceof String s) {
            return id(s);
        }
        if (id instanceof UUID u) {
            return id(u);
        }
        throw new IllegalArgumentException("loams: a document id is a number, a string or a UUID");
    }

    /** A document id as text: the number, the string or the UUID. */
    public static String idText(DocumentId id) {
        return switch (id.getIdCase()) {
            case UINT -> Long.toUnsignedString(id.getUint());
            case STRING -> id.getString();
            case UUID -> id.getUuid();
            default -> "";
        };
    }

    // `Query.ids` carries the `DocumentId` shape as a Value: {"uint": "1"} (exact past 2^53, and the
    // builder corpus's spelling), a string, or {"uuid": "…"}.
    static Value documentIdValue(DocumentId id) {
        return switch (id.getIdCase()) {
            case UINT -> of(Map.of("uint", Long.toUnsignedString(id.getUint())));
            case STRING -> Value.newBuilder().setStringValue(id.getString()).build();
            case UUID -> of(Map.of("uuid", id.getUuid()));
            default -> throw new IllegalArgumentException("loams: a DocumentId must be set");
        };
    }

    static List<Object> asList(Collection<?> items) {
        return new ArrayList<>(items);
    }

    /** A {@link Value} as compact JSON text: what a JSON record a stream carries looks like. */
    public static String json(Value value) {
        StringBuilder out = new StringBuilder();
        writeJson(out, value);
        return out.toString();
    }

    private static void writeJson(StringBuilder out, Value value) {
        switch (value.getKindCase()) {
            case BOOL_VALUE -> out.append(value.getBoolValue());
            case NUMBER_VALUE -> {
                double d = value.getNumberValue();
                if (d == Math.rint(d) && Math.abs(d) < 1e15) {
                    out.append((long) d);
                } else {
                    out.append(d);
                }
            }
            case STRING_VALUE -> quote(out, value.getStringValue());
            case LIST_VALUE -> {
                out.append('[');
                List<Value> items = value.getListValue().getValuesList();
                for (int i = 0; i < items.size(); i++) {
                    if (i > 0) {
                        out.append(',');
                    }
                    writeJson(out, items.get(i));
                }
                out.append(']');
            }
            case STRUCT_VALUE -> {
                out.append('{');
                boolean first = true;
                // Keys sorted, so a value encodes to the same bytes however its map was built.
                for (Map.Entry<String, Value> entry : new java.util.TreeMap<>(value.getStructValue().getFieldsMap()).entrySet()) {
                    if (!first) {
                        out.append(',');
                    }
                    first = false;
                    quote(out, entry.getKey());
                    out.append(':');
                    writeJson(out, entry.getValue());
                }
                out.append('}');
            }
            default -> out.append("null");
        }
    }

    private static void quote(StringBuilder out, String text) {
        out.append('"');
        for (int i = 0; i < text.length(); i++) {
            char c = text.charAt(i);
            switch (c) {
                case '"' -> out.append("\\\"");
                case '\\' -> out.append("\\\\");
                case '\n' -> out.append("\\n");
                case '\r' -> out.append("\\r");
                case '\t' -> out.append("\\t");
                default -> {
                    if (c < 0x20) {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
                }
            }
        }
        out.append('"');
    }
}
