package dev.loams;

import com.google.protobuf.ByteString;
import com.google.protobuf.NullValue;
import dev.loams.gen.loams.graph.v1.ListValue;
import dev.loams.gen.loams.graph.v1.MapValue;
import dev.loams.gen.loams.graph.v1.Value;
import dev.loams.gen.loams.graph.v1.Vector;
import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Plain Java values as {@code loams.graph.v1.Value}, for GQL parameters and results. */
public final class GraphValues {

    private GraphValues() {}

    /** A graph value: null, Boolean, Integer/Long, Float/Double, String, byte[], float[] (a vector), BigDecimal, List or Map. */
    public static Value of(Object value) {
        if (value == null) {
            return Value.newBuilder().setNull(NullValue.NULL_VALUE).build();
        }
        if (value instanceof Value v) {
            return v;
        }
        if (value instanceof Boolean b) {
            return Value.newBuilder().setBoolean(b).build();
        }
        if (value instanceof String s) {
            return Value.newBuilder().setString(s).build();
        }
        if (value instanceof Integer || value instanceof Long || value instanceof Short || value instanceof Byte) {
            return Value.newBuilder().setInt64(((Number) value).longValue()).build();
        }
        if (value instanceof Float || value instanceof Double) {
            return Value.newBuilder().setFloat64(((Number) value).doubleValue()).build();
        }
        if (value instanceof BigDecimal d) {
            return Value.newBuilder()
                    .setDecimal(dev.loams.gen.loams.graph.v1.Decimal.newBuilder().setValue(d.toPlainString()))
                    .build();
        }
        if (value instanceof byte[] bytes) {
            return Value.newBuilder().setBytes(ByteString.copyFrom(bytes)).build();
        }
        if (value instanceof float[] floats) {
            Vector.Builder vector = Vector.newBuilder();
            for (float f : floats) {
                vector.addValues(f);
            }
            return Value.newBuilder().setVector(vector).build();
        }
        if (value instanceof Map<?, ?> map) {
            MapValue.Builder out = MapValue.newBuilder();
            map.forEach((k, v) -> out.putEntries(String.valueOf(k), of(v)));
            return Value.newBuilder().setMap(out).build();
        }
        if (value instanceof Iterable<?> items) {
            ListValue.Builder out = ListValue.newBuilder();
            for (Object item : items) {
                out.addValues(of(item));
            }
            return Value.newBuilder().setList(out).build();
        }
        throw new IllegalArgumentException("loams: cannot send a " + value.getClass().getSimpleName() + " as a graph value");
    }

    /** A graph value as a plain Java value. */
    public static Object toPlain(Value value) {
        if (value == null) {
            return null;
        }
        return switch (value.getKindCase()) {
            case BOOLEAN -> value.getBoolean();
            case INT64 -> value.getInt64();
            case UINT64 -> value.getUint64();
            case FLOAT64 -> value.getFloat64();
            case STRING -> value.getString();
            case BYTES -> value.getBytes().toByteArray();
            case DECIMAL -> new BigDecimal(value.getDecimal().getValue());
            case LIST -> {
                List<Object> out = new ArrayList<>();
                for (Value item : value.getList().getValuesList()) {
                    out.add(toPlain(item));
                }
                yield out;
            }
            case MAP -> {
                Map<String, Object> out = new LinkedHashMap<>();
                value.getMap().getEntriesMap().forEach((k, v) -> out.put(k, toPlain(v)));
                yield out;
            }
            case NULL, KIND_NOT_SET -> null;
            default -> value;
        };
    }
}
