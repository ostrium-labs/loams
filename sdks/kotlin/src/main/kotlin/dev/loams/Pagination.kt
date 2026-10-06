package dev.loams

import com.google.protobuf.ByteString
import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage

/**
 * An iterator over a paged call's items, following `next_page_token` (R6).
 *
 * ## Why this clause has no fixture, and none is possible
 *
 * `sdks/fixtures/manifest.json` says so in as many words: no paged RPC exists on any
 * server, so there is nothing to record, and `mock_status_list_is_not_paged` is the
 * recording that *pins the fact* — `ListApprovals` declares both `page_size` and
 * `next_page_token` and honours neither. The end-to-end half arrives with API1 Task 2,
 * and the clause is pinned against a stub in every language meanwhile.
 *
 * ## Why the field names are not the caller's
 *
 * They are read off the **response descriptor**, so a proto that renames `approvals` or
 * `next_page_token` changes this iterator rather than a list of strings in a test file.
 * A hardcoded `"approvals"` here would be a second copy of the schema, and it would be
 * a copy nobody updates — the failure is a silent empty list rather than an error.
 *
 * ## Why it stops on a repeated token
 *
 * A server that hands back the token it was given is broken — a proxy with a stale cache
 * will do it — and a `for (item in client.listAll())` that loops on it is an unbounded
 * request inside a loop nobody wrote a bound for. So a token already used **ends** the
 * walk, and [stoppedOnRepeatedToken] says why.
 *
 * @param fetch makes one call with the given page token and returns the response.
 * @param itemsField the response's repeated items field.
 * @param nextPageTokenField the response's `next_page_token` field.
 */
open class PageIterator<T : DynamicMessage>(
    private val fetch: (String) -> T,
    val itemsField: FieldDescriptor,
    val nextPageTokenField: FieldDescriptor,
) : Iterator<T>, Iterable<T> {
    override fun iterator(): Iterator<T> = this

    init {
        // Both fields are validated at construction rather than at the first `next()`:
        // an iterator that only fails once the caller has started a `for` loop reports
        // the failure from the least convenient place.
        require(itemsField.isRepeated) {
            "${itemsField.fullName} is not a repeated field, so it cannot be a page's items"
        }
        require(itemsField.containingType != null && nextPageTokenField.containingType == itemsField.containingType) {
            "${nextPageTokenField.fullName} and ${itemsField.fullName} are declared on different messages, " +
                "so they cannot be two fields of one page"
        }
    }

    private var pending: List<T> = emptyList()
    private var at = 0
    private var nextToken: String = ""
    private var started = false
    private var finished = false

    /** Every page token this iterator asked with, in order. */
    val requestedTokens: MutableList<String> = mutableListOf()

    /** Every page token a server handed back, in order. */
    val offeredTokens: MutableList<String> = mutableListOf()

    /** Whether the walk ended because a server repeated a token rather than because one was empty. */
    var stoppedOnRepeatedToken: Boolean = false
        private set

    override fun hasNext(): Boolean = advance()

    /**
     * Fills [pending] from the next page and says whether there is anything in it.
     *
     * Split out of [hasNext] so [SchemaPageIterator] can inherit the walking without
     * re-implementing it — which is the whole point of this class being `open`.
     */
    protected fun advance(): Boolean {
        if (at < pending.size) {
            return true
        }
        if (finished) {
            return false
        }
        val token = if (started) nextToken else ""
        if (started && token.isEmpty()) {
            finished = true
            return false
        }
        if (started && token in offeredTokens) {
            // A repeated token: the walk ends rather than asking again, and says why.
            stoppedOnRepeatedToken = true
            finished = true
            return false
        }
        requestedTokens.add(token)
        if (started) {
            offeredTokens.add(token)
        }

        val page = fetch(token)
        started = true

        @Suppress("UNCHECKED_CAST")
        pending = (page.getField(itemsField) as? List<T>) ?: emptyList()
        at = 0
        nextToken = page.getField(nextPageTokenField).toString()

        if (at < pending.size) {
            return true
        }
        // An empty page is normal: a server may return no items and a token to continue
        // from. Only an **empty token** ends the walk, so an empty page does not.
        return advance()
    }

    override fun next(): T {
        if (!advance()) {
            throw NoSuchElementException(
                if (stoppedOnRepeatedToken) {
                    "the page iterator stopped because the server returned a token it had already returned"
                } else {
                    "the page iterator is exhausted"
                }
            )
        }
        return pending[at++]
    }
}

/**
 * A page iterator over a call whose items and token field names come from the schema.
 *
 * As [PageIterator], with the two names resolved off [binding]'s response descriptor.
 * Used by a facade method whose `FacadeOptions.pagination` annotation says what it
 * pages on.
 */
class SchemaPageIterator<T : DynamicMessage> private constructor(
    fetch: (String) -> T,
    binding: CallBinding,
    items: com.google.protobuf.Descriptors.FieldDescriptor,
    token: com.google.protobuf.Descriptors.FieldDescriptor,
) : PageIterator<T>(fetch, items, token) {
    companion object {
        /**
         * A page iterator over a call whose items and token field names come from the
         * schema.
         *
         * A `private constructor` plus a factory rather than a public one taking a
         * binding, because the two field descriptors have to be resolved **before**
         * [PageIterator]'s `init` validates them — and a public constructor would have
         * the descriptors as two more parameters a caller could get wrong, which is the
         * hand-written pair of names this class exists to avoid.
         */
        fun <T : DynamicMessage> forBinding(fetch: (String) -> T, binding: CallBinding): SchemaPageIterator<T> {
            val fields = binding.paginationFields() ?: throw IllegalArgumentException(
                "${binding.rpc} declares no FacadeOptions.pagination annotation, so it has no page fields to " +
                    "iterate; a hand-written pair of field names here would be a second copy of the schema"
            )
            return SchemaPageIterator(fetch, binding, fields.first, fields.second)
        }
    }
}

/**
 * The proto3 JSON of a message, for a caller logging a request.
 *
 * Not part of the wire: a request body goes through [MessageCodec], and this is here so
 * a log line and a body are the same bytes rather than two formatters that disagree.
 */
fun requestJson(request: DynamicMessage): String = CompactJson.format(request)

/**
 * Sets a string field on a message builder, for a caller building a request by hand.
 *
 * A convenience over `setField(field, ByteString.copyFromUtf8(...))` for the string
 * fields a request is mostly made of; it names the field rather than a number, so a proto
 * that renumbers does not silently write the wrong field.
 */
fun DynamicMessage.Builder.setString(name: String, value: String) {
    val field = requireNotNull(descriptorForType.findFieldByName(name)) {
        "$name is not a field of ${descriptorForType.fullName}"
    }
    require(field.type == FieldDescriptor.Type.STRING) { "$name is a ${field.type}, not a string" }
    setField(field, value)
}