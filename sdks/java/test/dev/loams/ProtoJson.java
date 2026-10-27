package dev.loams;

import com.google.protobuf.Descriptors.FieldDescriptor;
import com.google.protobuf.ListValue;
import com.google.protobuf.Message;
import com.google.protobuf.NullValue;
import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import dev.loams.internal.Json;
import java.util.List;
import java.util.Map;

/**
 * Just enough of the proto3 JSON mapping to read the builder corpus's {@code expect} blocks in a
 * test: messages, repeated fields, enums by name, numbers (64-bit ones as strings or numbers),
 * {@code Struct}, {@code Value}, {@code ListValue} and {@code Empty}. {@code JsonFormat} would do
 * it, but it needs Gson, which is not on this SDK's classpath.
 */
final class ProtoJson {

    private ProtoJson() {}

    static Message parse(Message like, String json) {
        return read(like.newBuilderForType(), Json.parse(json));
    }

    static Message fromParsed(Message like, Object json) {
        return read(like.newBuilderForType(), json);
    }

    @SuppressWarnings("unchecked")
    private static Message read(Message.Builder builder, Object json) {
        String type = builder.getDescriptorForType().getFullName();
        switch (type) {
            case "google.protobuf.Value":
                return value(json);
            case "google.protobuf.Struct":
                return struct((Map<String, Object>) json);
            case "google.protobuf.ListValue":
                return list((List<Object>) json);
            case "google.protobuf.Empty":
                return builder.build();
            default:
                break;
        }
        for (Map.Entry<String, Object> entry : ((Map<String, Object>) json).entrySet()) {
            FieldDescriptor field = null;
            for (FieldDescriptor candidate : builder.getDescriptorForType().getFields()) {
                if (candidate.getJsonName().equals(entry.getKey()) || candidate.getName().equals(entry.getKey())) {
                    field = candidate;
                }
            }
            if (field == null) {
                throw new AssertionError("no field " + entry.getKey() + " in " + type);
            }
            if (field.isRepeated()) {
                for (Object item : (List<Object>) entry.getValue()) {
                    builder.addRepeatedField(field, scalar(builder, field, item));
                }
            } else {
                builder.setField(field, scalar(builder, field, entry.getValue()));
            }
        }
        return builder.build();
    }

    private static Object scalar(Message.Builder parent, FieldDescriptor field, Object json) {
        return switch (field.getJavaType()) {
            case MESSAGE -> read(parent.newBuilderForField(field), json);
            case ENUM -> field.getEnumType().findValueByName((String) json);
            case STRING -> json;
            case BOOLEAN -> json;
            case INT -> json instanceof String s ? Integer.parseInt(s) : ((Double) json).intValue();
            case LONG -> json instanceof String s ? Long.parseUnsignedLong(s) : ((Double) json).longValue();
            case FLOAT -> ((Double) json).floatValue();
            case DOUBLE -> json;
            default -> throw new AssertionError("unsupported field " + field.getFullName());
        };
    }

    @SuppressWarnings("unchecked")
    private static Value value(Object json) {
        if (json == null) {
            return Value.newBuilder().setNullValue(NullValue.NULL_VALUE).build();
        }
        if (json instanceof Map<?, ?> map) {
            return Value.newBuilder().setStructValue(struct((Map<String, Object>) map)).build();
        }
        if (json instanceof List<?> items) {
            return Value.newBuilder().setListValue(list((List<Object>) items)).build();
        }
        if (json instanceof String s) {
            return Value.newBuilder().setStringValue(s).build();
        }
        if (json instanceof Boolean b) {
            return Value.newBuilder().setBoolValue(b).build();
        }
        return Value.newBuilder().setNumberValue((Double) json).build();
    }

    private static Struct struct(Map<String, Object> json) {
        Struct.Builder out = Struct.newBuilder();
        json.forEach((k, v) -> out.putFields(k, value(v)));
        return out.build();
    }

    private static ListValue list(List<Object> json) {
        ListValue.Builder out = ListValue.newBuilder();
        for (Object item : json) {
            out.addValues(value(item));
        }
        return out.build();
    }
}
