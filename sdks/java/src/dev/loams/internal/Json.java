package dev.loams.internal;

import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A small, strict JSON reader.
 *
 * <p>It exists so the SDK's runtime depends on nothing but {@code protobuf-java}. The SDK has
 * exactly one reason to read JSON — the Connect protocol's JSON error body, and the
 * {@code /oauth/token} response of {@link dev.loams.RefreshingTokenSource}'s OIDC exchange — and
 * taking a dependency on a JSON library to do it would put that library's version and its
 * security history into every Loams application's dependency tree for the sake of a few hundred
 * bytes of parsing.
 *
 * <p>It is <b>internal</b>: not part of the SDK's public surface, not covered by the
 * conformance suite's six names, and free to change. It parses RFC 8259 with the two extensions
 * JSON needs in practice and that {@code JSON.parse} in every language already allows by
 * default — nothing more. It does not accept trailing commas, comments, {@code NaN}, single
 * quotes or unquoted keys, because an error body is machine-generated and a lenient reader
 * would only hide a server bug.
 *
 * <p>Values come back as {@link Map} (insertion-ordered), {@link List}, {@link String},
 * {@link Double}, {@link Boolean} and {@code null}.
 */
public final class Json {

    private final String source;
    private int at;

    private Json(String source) {
        this.source = source;
    }

    /**
     * Parse one JSON document.
     *
     * @throws IllegalArgumentException when the text is not a single well-formed JSON value, or
     *     when anything other than whitespace follows it
     */
    public static Object parse(String text) {
        if (text == null) {
            throw new IllegalArgumentException("loams: cannot parse null as JSON");
        }
        Json reader = new Json(text);
        reader.skipWhitespace();
        Object value = reader.readValue(0);
        reader.skipWhitespace();
        if (reader.at != text.length()) {
            throw reader.fail("trailing content after the JSON value");
        }
        return value;
    }

    /** Parse a document that must be an object, as every Connect error body is. */
    @SuppressWarnings("unchecked")
    public static Map<String, Object> parseObject(String text) {
        Object value = parse(text);
        if (!(value instanceof Map)) {
            throw new IllegalArgumentException(
                    "loams: expected a JSON object, got " + typeName(value));
        }
        return (Map<String, Object>) value;
    }

    /** Parse a document that must be an array. */
    @SuppressWarnings("unchecked")
    public static List<Object> parseArray(String text) {
        Object value = parse(text);
        if (!(value instanceof List)) {
            throw new IllegalArgumentException(
                    "loams: expected a JSON array, got " + typeName(value));
        }
        return (List<Object>) value;
    }

    /**
     * The nesting limit.
     *
     * <p>A bound rather than the JDK's own: JSON.parse in another language will happily nest
     * until the stack runs out, and this reader is fed a server's error body, which is not
     * something the SDK controls. 64 is far deeper than any legal error body.
     */
    private static final int MAX_DEPTH = 64;

    private Object readValue(int depth) {
        if (depth > MAX_DEPTH) {
            throw fail("nested more than " + MAX_DEPTH + " deep");
        }
        char c = peek();
        switch (c) {
            case '{':
                return readObject(depth);
            case '[':
                return readArray(depth);
            case '"':
                return readString();
            case 't':
                readLiteral("true");
                return Boolean.TRUE;
            case 'f':
                readLiteral("false");
                return Boolean.FALSE;
            case 'n':
                readLiteral("null");
                return null;
            default:
                return readNumber();
        }
    }

    private Map<String, Object> readObject(int depth) {
        expect('{');
        Map<String, Object> out = new LinkedHashMap<>();
        skipWhitespace();
        if (peek() == '}') {
            at++;
            return out;
        }
        while (true) {
            skipWhitespace();
            String key = readString();
            skipWhitespace();
            expect(':');
            skipWhitespace();
            out.put(key, readValue(depth + 1));
            skipWhitespace();
            char c = next();
            if (c == '}') {
                return out;
            }
            if (c != ',') {
                throw fail("expected ',' or '}' in an object, got '" + c + "'");
            }
        }
    }

    private List<Object> readArray(int depth) {
        expect('[');
        List<Object> out = new ArrayList<>();
        skipWhitespace();
        if (peek() == ']') {
            at++;
            return Collections.unmodifiableList(out);
        }
        while (true) {
            skipWhitespace();
            out.add(readValue(depth + 1));
            skipWhitespace();
            char c = next();
            if (c == ']') {
                return Collections.unmodifiableList(out);
            }
            if (c != ',') {
                throw fail("expected ',' or ']' in an array, got '" + c + "'");
            }
        }
    }

    private String readString() {
        expect('"');
        StringBuilder out = new StringBuilder();
        while (true) {
            char c = next();
            if (c == '"') {
                return out.toString();
            }
            if (c != '\\') {
                if (c < 0x20) {
                    throw fail("a raw control character in a string");
                }
                out.append(c);
                continue;
            }
            char escape = next();
            switch (escape) {
                case '"':
                    out.append('"');
                    break;
                case '\\':
                    out.append('\\');
                    break;
                case '/':
                    out.append('/');
                    break;
                case 'b':
                    out.append('\b');
                    break;
                case 'f':
                    out.append('\f');
                    break;
                case 'n':
                    out.append('\n');
                    break;
                case 'r':
                    out.append('\r');
                    break;
                case 't':
                    out.append('\t');
                    break;
                case 'u':
                    out.append(readUnicodeEscape());
                    break;
                default:
                    throw fail("unknown string escape '\\" + escape + "'");
            }
        }
    }

    private char readUnicodeEscape() {
        if (at + 4 > source.length()) {
            throw fail("a truncated \\u escape");
        }
        int value = 0;
        for (int i = 0; i < 4; i++) {
            int digit = Character.digit(source.charAt(at + i), 16);
            if (digit < 0) {
                throw fail("a non-hex digit in a \\u escape");
            }
            value = (value << 4) | digit;
        }
        at += 4;
        // A surrogate pair arrives as two escapes; Java holds it as two chars, which is
        // correct, so nothing is joined here.
        return (char) value;
    }

    /**
     * A JSON number, with RFC 8259's grammar rather than a permissive scan.
     *
     * <p>The grammar is enforced because a lenient number reader is how a reader ends up
     * accepting {@code 01}, {@code Infinity} or {@code NaN} — values the server cannot have sent,
     * so accepting them would hide a server bug rather than accommodate one. The grammar is
     * {@code -?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?}.
     */
    private Double readNumber() {
        int start = at;
        if (peek() == '-') {
            at++;
        }
        // int: `0` alone, or a digit 1-9 followed by any digits.
        if (peek() == '0') {
            at++;
            if (isDigit(peek())) {
                throw fail("a number with a leading zero");
            }
        } else if (isNonZeroDigit(peek())) {
            while (isDigit(peek())) {
                at++;
            }
        } else {
            throw fail("expected a JSON value, got '" + peek() + "'");
        }
        // frac
        if (peek() == '.') {
            at++;
            if (!isDigit(peek())) {
                throw fail("a number with no digits after its point");
            }
            while (isDigit(peek())) {
                at++;
            }
        }
        // exp
        if (peek() == 'e' || peek() == 'E') {
            at++;
            if (peek() == '+' || peek() == '-') {
                at++;
            }
            if (!isDigit(peek())) {
                throw fail("a number with no digits in its exponent");
            }
            while (isDigit(peek())) {
                at++;
            }
        }
        return Double.valueOf(source.substring(start, at));
    }

    private static boolean isDigit(char c) {
        return c >= '0' && c <= '9';
    }

    private static boolean isNonZeroDigit(char c) {
        return c >= '1' && c <= '9';
    }

    private void readLiteral(String literal) {
        if (!source.startsWith(literal, at)) {
            throw fail("expected '" + literal + "'");
        }
        at += literal.length();
    }

    private void skipWhitespace() {
        while (at < source.length()) {
            char c = source.charAt(at);
            if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
                at++;
            } else {
                return;
            }
        }
    }

    private char peek() {
        if (at >= source.length()) {
            throw fail("the JSON ended early");
        }
        return source.charAt(at);
    }

    private char next() {
        char c = peek();
        at++;
        return c;
    }

    private void expect(char expected) {
        char c = next();
        if (c != expected) {
            throw fail("expected '" + expected + "', got '" + c + "'");
        }
    }

    private IllegalArgumentException fail(String what) {
        return new IllegalArgumentException("loams: " + what + " at offset " + at);
    }

    private static String typeName(Object value) {
        if (value == null) {
            return "null";
        }
        if (value instanceof Map) {
            return "an object";
        }
        if (value instanceof List) {
            return "an array";
        }
        if (value instanceof String) {
            return "a string";
        }
        if (value instanceof Boolean) {
            return "a boolean";
        }
        return "a number";
    }

    // Small typed readers, so a caller does not cast at every hop.

    /** The string at {@code key}, or {@code null} when absent or not a string. */
    public static String string(Map<String, Object> object, String key) {
        Object value = object.get(key);
        return value instanceof String ? (String) value : null;
    }

    /**
     * The string at {@code key}, or {@code fallback}.
     *
     * <p>Used for optional fields where a missing value and an empty one mean the same thing —
     * a {@code hint} the server left out and one it left blank both carry no instruction.
     */
    public static String string(Map<String, Object> object, String key, String fallback) {
        String value = string(object, key);
        return value == null ? fallback : value;
    }

    /** The object at {@code key}, or {@code null}. */
    @SuppressWarnings("unchecked")
    public static Map<String, Object> object(Map<String, Object> object, String key) {
        Object value = object.get(key);
        return value instanceof Map ? (Map<String, Object>) value : null;
    }

    /** The array at {@code key}, or an empty list. */
    @SuppressWarnings("unchecked")
    public static List<Object> array(Map<String, Object> object, String key) {
        Object value = object.get(key);
        return value instanceof List ? (List<Object>) value : List.of();
    }

    /**
     * The object at {@code key} as a string map, or an empty map.
     *
     * <p>A protobuf {@code map<string, string>} is a plain JSON object in the proto3 JSON
     * mapping, so {@code ErrorInfo.metadata} arrives as {@code {"variant":"standard"}} rather
     * than as a list of pairs. An entry whose value is not a string is skipped rather than
     * failing the whole error: one malformed entry from a newer server should not cost the
     * caller the metadata on the others (R8).
     */
    public static Map<String, String> stringMap(Map<String, Object> object, String key) {
        Map<String, Object> nested = object(object, key);
        if (nested == null) {
            return Map.of();
        }
        Map<String, String> out = new LinkedHashMap<>();
        for (Map.Entry<String, Object> entry : nested.entrySet()) {
            if (entry.getValue() instanceof String value) {
                out.put(entry.getKey(), value);
            }
        }
        return out;
    }
}