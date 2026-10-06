package dev.loams

/**
 * A JSON value, and a reader and writer for it.
 *
 * ## Why this is here rather than a dependency
 *
 * The SDK reads JSON in exactly three places — a Connect error body, a Connect
 * stream's end-of-stream envelope, and a gRPC-Web trailers frame — and writes it
 * in exactly one, the compact proto3 JSON mapping a request body is. Taking a JSON
 * library for that would put its version and its security history into every Loams
 * application's dependency tree for the sake of a few hundred bytes of parsing.
 *
 * The **writer** is the stricter half and lives in [CompactJson], because it has to
 * produce byte-exact proto3 JSON. This file's reader is a correctness question
 * (it must refuse what it does not understand rather than half-parse it) and the
 * writer is a conformance one.
 *
 * ## Nothing here throws
 *
 * [parse] returns `null` for anything it will not accept. A refusal to parse a
 * server's body is not an exceptional condition: a proxy answers
 * `502 Bad Gateway` with an HTML page, and that has to become a typed failure
 * rather than an exception out of the response reader.
 */
sealed class JsonValue {
    /** This value as an object, or null when it is not one. */
    fun asObject(): Json.JsonObject? = this as? Json.JsonObject

    /** This value's elements, or null when it is not an array. */
    fun asArray(): List<JsonValue>? = (this as? Json.JsonArray)?.items

    /** This value as a string, or null when it is not one. */
    fun asString(): String? = (this as? Json.JsonString)?.value

    /** This value as a number, or null when it is not one. A JSON number is a double to most readers. */
    fun asLong(): Long? = when (this) {
        is Json.JsonNumber -> raw.toDoubleOrNull()?.toLong()
        else -> null
    }

    /** This value as a boolean, or null when it is not one. */
    fun asBoolean(): Boolean? = (this as? Json.JsonBoolean)?.value

    /** This value when it is JSON `null`, and null otherwise — so `asObject()` and this cannot be confused. */
    fun isNull(): Boolean = this is Json.JsonNull

    // Kotlin sees a `fun asObject(): T?` as a *method*, so `value.obj` does not
    // read it — and every call site that wrote it without parentheses failed to compile.
    // Kotlin has no way to expose one accessor as both a property and a method, so these
    // four properties are the spelling that reads correctly, and the methods above stay
    // for the places that already call them. One accessor, two spellings, both
    // delegating to the same `as` cast: there is no logic to disagree about.

    /** This value as an object, or null. See [asObject]. */
    val obj: Json.JsonObject? get() = asObject()

    /** This value's elements, or null. See [asArray]. */
    val array: List<JsonValue>? get() = asArray()

    /** This value as a string, or null. See [asString]. */
    val string: String? get() = asString()

    /** This value as a boolean, or null. See [asBoolean]. */
    val boolean: Boolean? get() = asBoolean()

    /** This value as a number, or null. See [asLong]. */
    val long: Long? get() = asLong()

    /** This value as a double, or null. */
    val double: Double? get() = when (this) {
        is Json.JsonNumber -> raw.toDoubleOrNull()
        is Json.JsonString -> value.toDoubleOrNull()
        else -> null
    }
}

/**
 * A JSON reader and writer.
 *
 * Strict by construction. The rules it enforces and each is a way a wrong answer
 * becomes a wrong *reason*:
 *
 *  - an object needs `"` and `:` around every member name and value;
 *  - a trailing comma is an error, not a tolerated extension — it is what
 *    `JSON.parse` in JavaScript refuses and what the corpus never emits;
 *  - a literal must be spelled in full (`true`, not `tru`);
 *  - a number may not have a leading zero, a leading `+`, or a bare `.`;
 *  - strings reject a control character below `0x20` unescaped and an unterminated
 *    escape.
 *
 * Depth is bounded, because a body is untrusted input and a recursive-descent
 * parser over one is a stack-overflow crash rather than a failure.
 */
object Json {
    /** The nesting depth this reader accepts, before it refuses. */
    const val MAX_DEPTH = 64

    /** A JSON object, and its members in the order the document had them. */
    class JsonObject(val entries: Map<String, JsonValue>) : JsonValue() {
        /** The member called [name], or null. */
        operator fun get(name: String): JsonValue? = entries[name]

        /** The member called [name] as a string, or null. */
        fun member(name: String): String? = entries[name]?.asString()

        /** The member called [name] as a boolean, or null. */
        fun flag(name: String): Boolean? = entries[name]?.asBoolean()

        override fun equals(other: Any?): Boolean = this === other || (other is JsonObject && entries == other.entries)

        override fun hashCode(): Int = entries.hashCode()

        override fun toString(): String = writeCompact(this)
    }

    /** A JSON array. */
    class JsonArray(val items: List<JsonValue>) : JsonValue() {
        override fun equals(other: Any?): Boolean = this === other || (other is JsonArray && items == other.items)

        override fun hashCode(): Int = items.hashCode()

        override fun toString(): String = writeCompact(this)
    }

    /** A JSON string. */
    class JsonString(val value: String) : JsonValue() {
        override fun equals(other: Any?): Boolean = this === other || (other is JsonString && value == other.value)

        override fun hashCode(): Int = value.hashCode()

        override fun toString(): String = writeCompact(this)
    }

    /**
     * A JSON number, kept as the text that was written.
     *
     * The raw text rather than a `Double`, because `revision` is a `uint64` and a
     * double cannot hold it: `9007199254740993` round-tripped through one comes
     * back as `9007199254740992`, which is a different revision. The corpus's
     * `expect.revision` is a **string** for the same reason.
     */
    class JsonNumber(val raw: String) : JsonValue() {
        override fun equals(other: Any?): Boolean = this === other || (other is JsonNumber && raw == other.raw)

        override fun hashCode(): Int = raw.hashCode()

        override fun toString(): String = raw
    }

    /** A JSON `true` or `false`. */
    class JsonBoolean(val value: Boolean) : JsonValue() {
        override fun equals(other: Any?): Boolean = this === other || (other is JsonBoolean && value == other.value)

        override fun hashCode(): Int = value.hashCode()

        override fun toString(): String = if (value) "true" else "false"
    }

    /** JSON `null`. */
    object JsonNull : JsonValue() {
        override fun toString(): String = "null"
    }

    /** Parses [text], or returns null for anything this reader will not accept. */
    fun parse(text: String): JsonValue? = try {
        val parser = Parser(text)
        val value = parser.value(0)
        parser.skipWhitespace()
        if (parser.atEnd) value else null
    } catch (_: Refused) {
        null
    }

    /** Parses [text], throwing when it will not parse. For tests and for fixtures. */
    fun parseOrThrow(text: String): JsonValue = parse(text) ?: throw IllegalArgumentException("not JSON: $text")

    /** The compact form: no whitespace between tokens. */
    fun writeCompact(value: JsonValue): String = StringBuilder().also { write(value, it, pretty = false) }.toString()

    /** The indented form, for a file somebody reads. */
    fun writePretty(value: JsonValue): String = StringBuilder().also { write(value, it, pretty = true) }.toString()

    /** Thrown internally on anything the reader refuses; [parse] turns it into null. */
    private class Refused : Exception(null, null, false, false)

    private class Parser(private val text: String) {
        var at = 0

        val atEnd: Boolean get() = at >= text.length

        fun skipWhitespace() {
            while (at < text.length) {
                when (text[at]) {
                    ' ', '\t', '\n', '\r' -> at++
                    else -> return
                }
            }
        }

        fun value(depth: Int): JsonValue {
            if (depth > MAX_DEPTH) throw Refused()
            skipWhitespace()
            if (atEnd) throw Refused()
            return when (val c = text[at]) {
                '{' -> objectValue(depth)
                '[' -> arrayValue(depth)
                '"' -> JsonString(string())
                't' -> literal("true", JsonBoolean(true))
                'f' -> literal("false", JsonBoolean(false))
                'n' -> literal("null", JsonNull)
                else -> if (c == '-' || c in '0'..'9') number() else throw Refused()
            }
        }

        fun literal(word: String, value: JsonValue): JsonValue {
            if (!text.startsWith(word, at)) throw Refused()
            at += word.length
            return value
        }

        fun objectValue(depth: Int): JsonObject {
            at++ // '{'
            val entries = LinkedHashMap<String, JsonValue>()
            skipWhitespace()
            if (!atEnd && text[at] == '}') {
                at++
                return JsonObject(entries)
            }
            while (true) {
                skipWhitespace()
                if (atEnd || text[at] != '"') throw Refused()
                val name = string()
                skipWhitespace()
                if (atEnd || text[at] != ':') throw Refused()
                at++
                // A duplicate member name is a refusal rather than last-one-wins: a
                // body with two `reason` members has no single reason, and picking
                // one would report a reason the server did not mean.
                if (entries.containsKey(name)) throw Refused()
                entries[name] = value(depth + 1)
                skipWhitespace()
                if (atEnd) throw Refused()
                when (text[at]) {
                    ',' -> at++
                    '}' -> {
                        at++
                        return JsonObject(entries)
                    }
                    else -> throw Refused()
                }
            }
        }

        fun arrayValue(depth: Int): JsonArray {
            at++ // '['
            val items = mutableListOf<JsonValue>()
            skipWhitespace()
            if (!atEnd && text[at] == ']') {
                at++
                return JsonArray(items)
            }
            while (true) {
                items.add(value(depth + 1))
                skipWhitespace()
                if (atEnd) throw Refused()
                when (text[at]) {
                    ',' -> at++
                    ']' -> {
                        at++
                        return JsonArray(items)
                    }
                    else -> throw Refused()
                }
            }
        }

        fun string(): String {
            at++ // '"'
            val out = StringBuilder()
            while (true) {
                if (atEnd) throw Refused()
                when (val c = text[at]) {
                    '"' -> {
                        at++
                        return out.toString()
                    }
                    '\\' -> {
                        at++
                        if (atEnd) throw Refused()
                        when (val escape = text[at]) {
                            '"' -> out.append('"')
                            '\\' -> out.append('\\')
                            '/' -> out.append('/')
                            'b' -> out.append('\b')
                            'f' -> out.append('')
                            'n' -> out.append('\n')
                            'r' -> out.append('\r')
                            't' -> out.append('\t')
                            'u' -> {
                                if (at + 4 >= text.length) throw Refused()
                                val digits = text.substring(at + 1, at + 5)
                                val code = digits.toIntOrNull(16) ?: throw Refused()
                                if (code < 0xD800 || code > 0xDFFF) throw Refused()
                                // A high surrogate must be followed by its low
                                // partner. An unpaired surrogate is not a character
                                // any JSON consumer can encode, so accepting it would
                                // produce a body that cannot be sent.
                                if (code in 0xD800..0xDBFF) {
                                    if (!text.startsWith("\\u", at + 5)) throw Refused()
                                    val low = text.substring(at + 6, at + 10).toIntOrNull(16) ?: throw Refused()
                                    if (low !in 0xDC00..0xDFFF) throw Refused()
                                    out.append(code.toChar())
                                    out.append(low.toChar())
                                    at += 10
                                    continue
                                }
                                out.append(code.toChar())
                                at += 4
                                continue
                            }
                            else -> throw Refused()
                        }
                        at++
                    }
                    else -> {
                        if (c.code < 0x20) throw Refused()
                        out.append(c)
                        at++
                    }
                }
            }
        }

        fun number(): JsonNumber {
            val start = at
            if (!atEnd && text[at] == '-') at++
            if (atEnd) throw Refused()
            when (text[at]) {
                '0' -> {
                    at++
                    if (!atEnd && text[at] in '0'..'9') throw Refused()
                }
                in '1'..'9' -> {
                    while (!atEnd && text[at] in '0'..'9') at++
                }
                else -> throw Refused()
            }
            if (!atEnd && text[at] == '.') {
                at++
                if (atEnd || text[at] !in '0'..'9') throw Refused()
                while (!atEnd && text[at] in '0'..'9') at++
            }
            if (!atEnd && (text[at] == 'e' || text[at] == 'E')) {
                at++
                if (!atEnd && (text[at] == '+' || text[at] == '-')) at++
                if (atEnd || text[at] !in '0'..'9') throw Refused()
                while (!atEnd && text[at] in '0'..'9') at++
            }
            return JsonNumber(text.substring(start, at))
        }
    }

    private fun write(value: JsonValue, out: StringBuilder, pretty: Boolean, depth: Int = 0) {
        when (value) {
            is JsonNull -> out.append("null")
            is JsonBoolean -> out.append(if (value.value) "true" else "false")
            is JsonNumber -> out.append(value.raw)
            is JsonString -> writeString(value.value, out)
            is JsonArray -> {
                out.append('[')
                value.items.forEachIndexed { index, item ->
                    if (index > 0) out.append(',')
                    if (pretty) newline(out, depth + 1)
                    write(item, out, pretty, depth + 1)
                }
                if (pretty && value.items.isNotEmpty()) newline(out, depth)
                out.append(']')
            }
            is JsonObject -> {
                out.append('{')
                var first = true
                for ((name, member) in value.entries) {
                    if (!first) out.append(',')
                    first = false
                    if (pretty) newline(out, depth + 1)
                    writeString(name, out)
                    out.append(':')
                    if (pretty) out.append(' ')
                    write(member, out, pretty, depth + 1)
                }
                if (pretty && value.entries.isNotEmpty()) newline(out, depth)
                out.append('}')
            }
        }
    }

    private fun newline(out: StringBuilder, depth: Int) {
        out.append('\n')
        repeat(depth) { out.append("  ") }
    }

    /** Writes a JSON string, escaping exactly what RFC 8259 requires and nothing else. */
    private fun writeString(value: String, out: StringBuilder) {
        out.append('"')
        for (c in value) {
            when {
                c == '"' -> out.append("\\\"")
                c == '\\' -> out.append("\\\\")
                c == '\b' -> out.append("\\b")
                c == '' -> out.append("\\f")
                c == '\n' -> out.append("\\n")
                c == '\r' -> out.append("\\r")
                c == '\t' -> out.append("\\t")
                c.code < 0x20 -> out.append("\\u").append("%04x".format(c.code))
                else -> out.append(c)
            }
        }
        out.append('"')
    }
}