package dev.loams.internal;

import com.google.protobuf.ByteString;
import com.google.protobuf.Descriptors.Descriptor;
import com.google.protobuf.Descriptors.EnumValueDescriptor;
import com.google.protobuf.Descriptors.FieldDescriptor;
import com.google.protobuf.Message;
import com.google.protobuf.MessageOrBuilder;
import java.math.BigDecimal;
import java.time.Instant;
import java.time.format.DateTimeFormatter;
import java.util.Base64;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * The proto3 JSON mapping, for {@code Options.json(true)}.
 *
 * <p>{@code JsonFormat} in {@code protobuf-java-util} does this, but it needs Gson, and the SDK's
 * only runtime dependency is {@code protobuf-java}. So this is the mapping written against the
 * descriptors: field names in lowerCamelCase, 64-bit integers as strings, {@code bytes} as base64,
 * enums by name, maps as objects, fields at their default left out (unless they track presence),
 * and the well-known types the Loams API uses ({@code Struct}, {@code Value}, {@code ListValue},
 * {@code Empty}, {@code Timestamp}, {@code Duration}, {@code FieldMask} and the wrappers). A
 * reader takes either the JSON name or the proto name, and skips a field it does not know, so an
 * answer from a newer server still reads.
 */
public final class ProtoJsonCodec {

    private ProtoJsonCodec() {}

    /** A message as compact proto3 JSON. */
    public static String print(MessageOrBuilder message) {
        StringBuilder out = new StringBuilder();
        writeMessage(out, message);
        return out.toString();
    }

    /**
     * Read proto3 JSON into {@code builder}.
     *
     * @throws IllegalArgumentException when the text is not JSON or does not fit the message
     */
    public static <B extends Message.Builder> B merge(String json, B builder) {
        String text = json.trim();
        mergeValue(text.isEmpty() ? Map.of() : Json.parse(text), builder);
        return builder;
    }

    /** Read an already-parsed JSON value ({@link Json#parse}'s maps, lists and scalars) into {@code builder}. */
    public static <B extends Message.Builder> B mergeParsed(Object json, B builder) {
        mergeValue(json == null ? Map.of() : json, builder);
        return builder;
    }

    // ---------------------------------------------------------------- writing

    private static void writeMessage(StringBuilder out, MessageOrBuilder message) {
        Descriptor type = message.getDescriptorForType();
        switch (type.getFullName()) {
            case "google.protobuf.Value" -> {
                writeValue(out, message);
                return;
            }
            case "google.protobuf.Struct" -> {
                writeStruct(out, message);
                return;
            }
            case "google.protobuf.ListValue" -> {
                writeList(out, message);
                return;
            }
            case "google.protobuf.Timestamp" -> {
                long seconds = (Long) message.getField(type.findFieldByName("seconds"));
                int nanos = (Integer) message.getField(type.findFieldByName("nanos"));
                quote(out, DateTimeFormatter.ISO_INSTANT.format(Instant.ofEpochSecond(seconds, nanos)));
                return;
            }
            case "google.protobuf.Duration" -> {
                long seconds = (Long) message.getField(type.findFieldByName("seconds"));
                int nanos = (Integer) message.getField(type.findFieldByName("nanos"));
                BigDecimal value = BigDecimal.valueOf(seconds).add(BigDecimal.valueOf(nanos, 9));
                quote(out, value.stripTrailingZeros().toPlainString() + "s");
                return;
            }
            case "google.protobuf.FieldMask" -> {
                @SuppressWarnings("unchecked")
                List<String> paths = (List<String>) message.getField(type.findFieldByName("paths"));
                StringBuilder joined = new StringBuilder();
                for (String path : paths) {
                    if (joined.length() > 0) {
                        joined.append(',');
                    }
                    joined.append(lowerCamel(path));
                }
                quote(out, joined.toString());
                return;
            }
            case "google.protobuf.DoubleValue", "google.protobuf.FloatValue", "google.protobuf.Int64Value",
                    "google.protobuf.UInt64Value", "google.protobuf.Int32Value", "google.protobuf.UInt32Value",
                    "google.protobuf.BoolValue", "google.protobuf.StringValue", "google.protobuf.BytesValue" -> {
                FieldDescriptor field = type.findFieldByName("value");
                writeSingular(out, field, message.getField(field));
                return;
            }
            case "google.protobuf.Any" ->
                throw new IllegalArgumentException("loams: google.protobuf.Any has no JSON mapping in this SDK");
            default -> {}
        }
        out.append('{');
        boolean first = true;
        for (FieldDescriptor field : type.getFields()) {
            if (field.isMapField()) {
                List<?> entries = (List<?>) message.getField(field);
                if (entries.isEmpty()) {
                    continue;
                }
                first = name(out, field, first);
                writeMap(out, field, entries);
                continue;
            }
            if (field.isRepeated()) {
                int count = message.getRepeatedFieldCount(field);
                if (count == 0) {
                    continue;
                }
                first = name(out, field, first);
                out.append('[');
                for (int i = 0; i < count; i++) {
                    if (i > 0) {
                        out.append(',');
                    }
                    writeSingular(out, field, message.getRepeatedField(field, i));
                }
                out.append(']');
                continue;
            }
            if (field.hasPresence()) {
                if (!message.hasField(field)) {
                    continue;
                }
            } else if (message.getField(field).equals(field.getDefaultValue())) {
                continue;
            }
            first = name(out, field, first);
            writeSingular(out, field, message.getField(field));
        }
        out.append('}');
    }

    private static boolean name(StringBuilder out, FieldDescriptor field, boolean first) {
        if (!first) {
            out.append(',');
        }
        quote(out, field.getJsonName());
        out.append(':');
        return false;
    }

    private static void writeMap(StringBuilder out, FieldDescriptor field, List<?> entries) {
        FieldDescriptor keyField = field.getMessageType().findFieldByNumber(1);
        FieldDescriptor valueField = field.getMessageType().findFieldByNumber(2);
        // Keys sorted, so one map encodes to the same text however it was built.
        Map<String, Object> sorted = new TreeMap<>();
        for (Object entry : entries) {
            Message pair = (Message) entry;
            sorted.put(mapKey(keyField, pair.getField(keyField)), pair.getField(valueField));
        }
        out.append('{');
        boolean first = true;
        for (Map.Entry<String, Object> entry : sorted.entrySet()) {
            if (!first) {
                out.append(',');
            }
            first = false;
            quote(out, entry.getKey());
            out.append(':');
            writeSingular(out, valueField, entry.getValue());
        }
        out.append('}');
    }

    private static String mapKey(FieldDescriptor key, Object value) {
        return switch (key.getType()) {
            case UINT32, FIXED32 -> Integer.toUnsignedString((Integer) value);
            case UINT64, FIXED64 -> Long.toUnsignedString((Long) value);
            default -> String.valueOf(value);
        };
    }

    private static void writeSingular(StringBuilder out, FieldDescriptor field, Object value) {
        switch (field.getType()) {
            case MESSAGE, GROUP -> writeMessage(out, (MessageOrBuilder) value);
            case ENUM -> {
                EnumValueDescriptor e = (EnumValueDescriptor) value;
                if (e.getType().getFullName().equals("google.protobuf.NullValue")) {
                    out.append("null");
                } else if (e.getIndex() >= 0) {
                    // A value this build knows by name; an unknown one (index -1) goes as its number.
                    quote(out, e.getName());
                } else {
                    out.append(e.getNumber());
                }
            }
            case INT32, SINT32, SFIXED32 -> out.append((int) (Integer) value);
            case UINT32, FIXED32 -> out.append(Integer.toUnsignedString((Integer) value));
            case INT64, SINT64, SFIXED64 -> quote(out, Long.toString((Long) value));
            case UINT64, FIXED64 -> quote(out, Long.toUnsignedString((Long) value));
            case FLOAT -> writeNumber(out, (Float) value, Float.toString((Float) value));
            case DOUBLE -> writeNumber(out, (Double) value, Double.toString((Double) value));
            case BOOL -> out.append((boolean) (Boolean) value);
            case STRING -> quote(out, (String) value);
            case BYTES -> quote(out, Base64.getEncoder().encodeToString(((ByteString) value).toByteArray()));
        }
    }

    private static void writeNumber(StringBuilder out, double value, String text) {
        if (Double.isNaN(value)) {
            quote(out, "NaN");
        } else if (Double.isInfinite(value)) {
            quote(out, value > 0 ? "Infinity" : "-Infinity");
        } else if (value == Math.rint(value) && Math.abs(value) < 1e15) {
            out.append((long) value);
        } else {
            out.append(text);
        }
    }

    private static void writeValue(StringBuilder out, MessageOrBuilder value) {
        Descriptor type = value.getDescriptorForType();
        FieldDescriptor set = value.getOneofFieldDescriptor(type.getOneofs().get(0));
        if (set == null) {
            out.append("null");
            return;
        }
        Object inner = value.getField(set);
        switch (set.getName()) {
            case "null_value" -> out.append("null");
            case "number_value" -> {
                double d = (Double) inner;
                if (Double.isNaN(d) || Double.isInfinite(d)) {
                    throw new IllegalArgumentException("loams: a google.protobuf.Value cannot hold " + d);
                }
                writeNumber(out, d, Double.toString(d));
            }
            case "string_value" -> quote(out, (String) inner);
            case "bool_value" -> out.append((boolean) (Boolean) inner);
            case "struct_value" -> writeStruct(out, (MessageOrBuilder) inner);
            case "list_value" -> writeList(out, (MessageOrBuilder) inner);
            default -> out.append("null");
        }
    }

    private static void writeStruct(StringBuilder out, MessageOrBuilder struct) {
        FieldDescriptor fields = struct.getDescriptorForType().findFieldByName("fields");
        writeMap(out, fields, (List<?>) struct.getField(fields));
    }

    private static void writeList(StringBuilder out, MessageOrBuilder list) {
        FieldDescriptor values = list.getDescriptorForType().findFieldByName("values");
        out.append('[');
        int count = list.getRepeatedFieldCount(values);
        for (int i = 0; i < count; i++) {
            if (i > 0) {
                out.append(',');
            }
            writeValue(out, (MessageOrBuilder) list.getRepeatedField(values, i));
        }
        out.append(']');
    }

    private static String lowerCamel(String snake) {
        StringBuilder out = new StringBuilder();
        boolean upper = false;
        for (char c : snake.toCharArray()) {
            if (c == '_') {
                upper = true;
            } else {
                out.append(upper ? Character.toUpperCase(c) : c);
                upper = false;
            }
        }
        return out.toString();
    }

    private static String snake(String camel) {
        StringBuilder out = new StringBuilder();
        for (char c : camel.toCharArray()) {
            if (Character.isUpperCase(c)) {
                out.append('_').append(Character.toLowerCase(c));
            } else {
                out.append(c);
            }
        }
        return out.toString();
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
                case '\b' -> out.append("\\b");
                case '\f' -> out.append("\\f");
                default -> {
                    if (c < 0x20 || c == ' ' || c == ' ') {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
                }
            }
        }
        out.append('"');
    }

    // ---------------------------------------------------------------- reading

    @SuppressWarnings("unchecked")
    private static void mergeValue(Object json, Message.Builder builder) {
        Descriptor type = builder.getDescriptorForType();
        switch (type.getFullName()) {
            case "google.protobuf.Value" -> {
                readValue(json, builder);
                return;
            }
            case "google.protobuf.Struct" -> {
                readStruct(expect(json, Map.class, type), builder);
                return;
            }
            case "google.protobuf.ListValue" -> {
                readList(expect(json, List.class, type), builder);
                return;
            }
            case "google.protobuf.Timestamp" -> {
                Instant at = Instant.parse(expect(json, String.class, type));
                builder.setField(type.findFieldByName("seconds"), at.getEpochSecond());
                builder.setField(type.findFieldByName("nanos"), at.getNano());
                return;
            }
            case "google.protobuf.Duration" -> {
                String text = expect(json, String.class, type);
                if (!text.endsWith("s")) {
                    throw new IllegalArgumentException("loams: a Duration in JSON ends in s, got " + text);
                }
                BigDecimal value = new BigDecimal(text.substring(0, text.length() - 1));
                long seconds = value.longValue();
                int nanos = value.subtract(BigDecimal.valueOf(seconds)).movePointRight(9).intValue();
                builder.setField(type.findFieldByName("seconds"), seconds);
                builder.setField(type.findFieldByName("nanos"), nanos);
                return;
            }
            case "google.protobuf.FieldMask" -> {
                String text = expect(json, String.class, type);
                FieldDescriptor paths = type.findFieldByName("paths");
                if (!text.isEmpty()) {
                    for (String path : text.split(",")) {
                        builder.addRepeatedField(paths, snake(path));
                    }
                }
                return;
            }
            case "google.protobuf.Empty" -> {
                return;
            }
            case "google.protobuf.DoubleValue", "google.protobuf.FloatValue", "google.protobuf.Int64Value",
                    "google.protobuf.UInt64Value", "google.protobuf.Int32Value", "google.protobuf.UInt32Value",
                    "google.protobuf.BoolValue", "google.protobuf.StringValue", "google.protobuf.BytesValue" -> {
                FieldDescriptor field = type.findFieldByName("value");
                builder.setField(field, readSingular(builder, field, json));
                return;
            }
            default -> {}
        }
        Map<String, Object> object = expect(json, Map.class, type);
        for (Map.Entry<String, Object> entry : object.entrySet()) {
            FieldDescriptor field = type.findFieldByName(entry.getKey());
            if (field == null) {
                for (FieldDescriptor candidate : type.getFields()) {
                    if (candidate.getJsonName().equals(entry.getKey())) {
                        field = candidate;
                        break;
                    }
                }
            }
            if (field == null) {
                // A field from a newer schema: skipped, so an older client still reads the rest.
                continue;
            }
            Object value = entry.getValue();
            if (value == null && !isValueField(field)) {
                continue;
            }
            if (field.isMapField()) {
                FieldDescriptor keyField = field.getMessageType().findFieldByNumber(1);
                FieldDescriptor valueField = field.getMessageType().findFieldByNumber(2);
                for (Map.Entry<String, Object> pair : ((Map<String, Object>) expect(value, Map.class, type)).entrySet()) {
                    Message.Builder entryBuilder = builder.newBuilderForField(field);
                    entryBuilder.setField(keyField, readMapKey(keyField, pair.getKey()));
                    entryBuilder.setField(valueField, readSingular(entryBuilder, valueField, pair.getValue()));
                    builder.addRepeatedField(field, entryBuilder.build());
                }
            } else if (field.isRepeated()) {
                for (Object item : (List<Object>) expect(value, List.class, type)) {
                    builder.addRepeatedField(field, readSingular(builder, field, item));
                }
            } else {
                builder.setField(field, readSingular(builder, field, value));
            }
        }
    }

    private static boolean isValueField(FieldDescriptor field) {
        return field.getJavaType() == FieldDescriptor.JavaType.MESSAGE
                && field.getMessageType().getFullName().equals("google.protobuf.Value");
    }

    @SuppressWarnings("unchecked")
    private static <T> T expect(Object json, Class<?> kind, Descriptor type) {
        if (!kind.isInstance(json)) {
            throw new IllegalArgumentException(
                    "loams: " + type.getFullName() + " in JSON is not a " + kind.getSimpleName() + ": " + json);
        }
        return (T) json;
    }

    private static Object readMapKey(FieldDescriptor key, String text) {
        return switch (key.getJavaType()) {
            case STRING -> text;
            case BOOLEAN -> Boolean.parseBoolean(text);
            case INT -> key.getType() == FieldDescriptor.Type.UINT32 || key.getType() == FieldDescriptor.Type.FIXED32
                    ? Integer.parseUnsignedInt(text) : Integer.parseInt(text);
            case LONG -> key.getType() == FieldDescriptor.Type.UINT64 || key.getType() == FieldDescriptor.Type.FIXED64
                    ? Long.parseUnsignedLong(text) : Long.parseLong(text);
            default -> throw new IllegalArgumentException("loams: unsupported map key " + key.getFullName());
        };
    }

    private static Object readSingular(Message.Builder parent, FieldDescriptor field, Object json) {
        switch (field.getType()) {
            case MESSAGE, GROUP -> {
                Message.Builder sub = parent.newBuilderForField(field);
                mergeValue(json, sub);
                return sub.build();
            }
            case ENUM -> {
                if (json == null) {
                    return field.getEnumType().findValueByNumber(0);
                }
                EnumValueDescriptor value = json instanceof String name
                        ? field.getEnumType().findValueByName(name)
                        : field.getEnumType().findValueByNumber(number(json).intValue());
                if (value == null) {
                    throw new IllegalArgumentException("loams: " + json + " is not a " + field.getEnumType().getFullName());
                }
                return value;
            }
            case INT32, SINT32, SFIXED32 -> {
                return number(json).intValueExact();
            }
            case UINT32, FIXED32 -> {
                long value = number(json).longValueExact();
                if (value < 0 || value > 0xFFFF_FFFFL) {
                    throw new IllegalArgumentException("loams: " + json + " is not a uint32");
                }
                return (int) value;
            }
            case INT64, SINT64, SFIXED64 -> {
                return number(json).longValueExact();
            }
            case UINT64, FIXED64 -> {
                BigDecimal value = number(json);
                if (value.signum() < 0) {
                    throw new IllegalArgumentException("loams: " + json + " is not a uint64");
                }
                return value.toBigIntegerExact().longValue();
            }
            case FLOAT -> {
                return (float) floating(json);
            }
            case DOUBLE -> {
                return floating(json);
            }
            case BOOL -> {
                if (json instanceof Boolean b) {
                    return b;
                }
                throw new IllegalArgumentException("loams: " + json + " is not a bool");
            }
            case STRING -> {
                if (json instanceof String s) {
                    return s;
                }
                throw new IllegalArgumentException("loams: " + json + " is not a string");
            }
            case BYTES -> {
                if (!(json instanceof String s)) {
                    throw new IllegalArgumentException("loams: " + json + " is not base64");
                }
                String standard = s.replace('-', '+').replace('_', '/');
                while (standard.length() % 4 != 0) {
                    standard += "=";
                }
                return ByteString.copyFrom(Base64.getDecoder().decode(standard));
            }
            default -> throw new IllegalArgumentException("loams: unsupported field " + field.getFullName());
        }
    }

    private static BigDecimal number(Object json) {
        if (json instanceof String s) {
            return new BigDecimal(s.trim());
        }
        if (json instanceof Double d) {
            return BigDecimal.valueOf(d);
        }
        if (json instanceof Number n) {
            return new BigDecimal(n.toString());
        }
        throw new IllegalArgumentException("loams: " + json + " is not a number");
    }

    private static double floating(Object json) {
        if (json instanceof String s) {
            return switch (s) {
                case "NaN" -> Double.NaN;
                case "Infinity" -> Double.POSITIVE_INFINITY;
                case "-Infinity" -> Double.NEGATIVE_INFINITY;
                default -> Double.parseDouble(s);
            };
        }
        if (json instanceof Number n) {
            return n.doubleValue();
        }
        throw new IllegalArgumentException("loams: " + json + " is not a number");
    }

    @SuppressWarnings("unchecked")
    private static void readValue(Object json, Message.Builder builder) {
        Descriptor type = builder.getDescriptorForType();
        if (json == null) {
            FieldDescriptor field = type.findFieldByName("null_value");
            builder.setField(field, field.getEnumType().findValueByNumber(0));
        } else if (json instanceof String s) {
            builder.setField(type.findFieldByName("string_value"), s);
        } else if (json instanceof Boolean b) {
            builder.setField(type.findFieldByName("bool_value"), b);
        } else if (json instanceof Number n) {
            builder.setField(type.findFieldByName("number_value"), n.doubleValue());
        } else if (json instanceof Map<?, ?> map) {
            FieldDescriptor field = type.findFieldByName("struct_value");
            Message.Builder struct = builder.newBuilderForField(field);
            readStruct((Map<String, Object>) map, struct);
            builder.setField(field, struct.build());
        } else if (json instanceof List<?> list) {
            FieldDescriptor field = type.findFieldByName("list_value");
            Message.Builder items = builder.newBuilderForField(field);
            readList((List<Object>) list, items);
            builder.setField(field, items.build());
        } else {
            throw new IllegalArgumentException("loams: " + json + " is not a JSON value");
        }
    }

    private static void readStruct(Map<String, Object> json, Message.Builder builder) {
        FieldDescriptor fields = builder.getDescriptorForType().findFieldByName("fields");
        FieldDescriptor valueField = fields.getMessageType().findFieldByNumber(2);
        for (Map.Entry<String, Object> pair : json.entrySet()) {
            Message.Builder entry = builder.newBuilderForField(fields);
            entry.setField(fields.getMessageType().findFieldByNumber(1), pair.getKey());
            Message.Builder value = entry.newBuilderForField(valueField);
            readValue(pair.getValue(), value);
            entry.setField(valueField, value.build());
            builder.addRepeatedField(fields, entry.build());
        }
    }

    private static void readList(List<Object> json, Message.Builder builder) {
        FieldDescriptor values = builder.getDescriptorForType().findFieldByName("values");
        for (Object item : json) {
            Message.Builder value = builder.newBuilderForField(values);
            readValue(item, value);
            builder.addRepeatedField(values, value.build());
        }
    }
}
