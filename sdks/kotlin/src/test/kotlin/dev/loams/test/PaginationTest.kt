package dev.loams.test

import com.google.protobuf.ByteString
import com.google.protobuf.DynamicMessage
import dev.loams.CallOptions
import dev.loams.ContentTypes
import dev.loams.Descriptors
import dev.loams.MessageCodec
import dev.loams.Codec
import dev.loams.PageIterator
import dev.loams.test.Support.StubTransport

/**
 * `kotlin_pagination_iterator` (R6).
 *
 * **This clause has no fixture and none is possible.** `manifest.json` says so in
 * as many words: no paged RPC exists on any server, so there is nothing to record,
 * and `mock_status_list_is_not_paged` is the recording that *pins the fact* —
 * `ListApprovals` declares both `page_size` and `next_page_token` and honours
 * neither. The end-to-end half arrives with API1 Task 2.
 *
 * So it is pinned here, against a stub serving the **real** `ListApprovalsRequest`
 * and `ListApprovalsResponse` types, and the fields the iterator reads are named by
 * the **schema** rather than written out — which is the point: the two field names
 * come off the descriptor, so a proto that renames `approvals` changes this
 * iterator rather than this test.
 */
object PaginationTest {
    const val NAME = "kotlin_pagination_iterator"

    internal val list = StubTransport.binding("loams.approvals.v1.ApprovalService/ListApprovals")

    /** A `ListApprovalsResponse` carrying [ids] and a `next_page_token`. */
    internal fun response(ids: List<String>, nextPageToken: String): DynamicMessage {
        val descriptor = list.response
        val builder = DynamicMessage.newBuilder(descriptor)
        val approvals = descriptor.findFieldByName("approvals")
        val approvalDescriptor = requireNotNull(approvals).messageType
        for (id in ids) {
            builder.addRepeatedField(
                approvals,
                DynamicMessage.newBuilder(approvalDescriptor)
                    .setField(approvalDescriptor.findFieldByName("id"), id)
                    .build(),
            )
        }
        builder.setField(descriptor.findFieldByName("next_page_token"), nextPageToken)
        return builder.build()
    }

    /** A `ListApprovalsRequest` naming a page size and a token. */
    private fun request(pageSize: Int = 0, pageToken: String = ""): DynamicMessage {
        val descriptor = list.request
        val builder = DynamicMessage.newBuilder(descriptor)
        if (pageSize > 0) {
            builder.setField(descriptor.findFieldByName("page_size"), java.lang.Integer.valueOf(pageSize))
        }
        if (pageToken.isNotEmpty()) {
            builder.setField(descriptor.findFieldByName("page_token"), pageToken)
        }
        return builder.build()
    }

    /** Every `page_size` and `page_token` the iterator sent, in order. */
    private fun pagesSent(transport: StubTransport): List<Pair<Int, String>> =
        transport.requests.map { sent ->
            val message = MessageCodec.deserialize(list.request, sent.body, Codec.PROTO)
            val size = (message.getField(list.request.findFieldByName("page_size")) as Number).toInt()
            val token = message.getField(list.request.findFieldByName("page_token")) as String
            size to token
        }

    fun register() {
        Harness.test("$NAME an iterator walks every page and stops on an empty token") {
            val transport = StubTransport()
            transport.answerMessage(list.response, response(listOf("apr_a", "apr_b"), "page-2"))
            transport.answerMessage(list.response, response(listOf("apr_c"), "page-3"))
            transport.answerMessage(list.response, response(listOf("apr_d"), "page-4"))
            transport.answerMessage(list.response, response(listOf("apr_e"), ""))

            val seen = mutableListOf<String>()
            val iterator = PageIterator(
                fetch = { pageToken ->
                    transport.client().invoker.unary(list, request(pageSize = 2, pageToken = pageToken), CallOptions())
                },
                itemsField = list.response.findFieldByName("approvals")!!,
                nextPageTokenField = list.response.findFieldByName("next_page_token")!!,
            )
            for (page in iterator) {
                seen.addAll(page.ids())
            }

            assertEquals(listOf("apr_a", "apr_b", "apr_c", "apr_d", "apr_e"), seen, "the ids across four pages")
            assertEquals(4, transport.requests.size, "pages fetched")
            val sent = pagesSent(transport)
            assertEquals(listOf(2 to "", 2 to "page-2", 2 to "page-3", 2 to "page-4"), sent, "the page_size and page_token of each request")
        }

        Harness.test("$NAME an empty first page yields nothing and asks for nothing more") {
            val transport = StubTransport()
            transport.answerMessage(list.response, response(emptyList(), ""))

            val seen = mutableListOf<String>()
            val iterator = PageIterator(
                fetch = { pageToken ->
                    transport.client().invoker.unary(list, request(pageToken = pageToken), CallOptions())
                },
                itemsField = list.response.findFieldByName("approvals")!!,
                nextPageTokenField = list.response.findFieldByName("next_page_token")!!,
            )
            for (page in iterator) {
                seen.addAll(page.ids())
            }
            assertEquals(emptyList<String>(), seen, "ids")
            assertEquals(1, transport.requests.size, "requests for an empty first page")
        }

        Harness.test("$NAME a page whose token repeats terminates rather than looping") {
            // A server that hands back the token it was given is broken, and a client
            // that loops on it is a client with an unbounded request in a `for`. The
            // iterator stops on a token it has already used, and says so.
            val transport = StubTransport()
            transport.answerMessage(list.response, response(listOf("apr_a"), "same"))
            transport.answerMessage(list.response, response(listOf("apr_b"), "same"))

            val seen = mutableListOf<String>()
            val iterator = PageIterator(
                fetch = { pageToken ->
                    transport.client().invoker.unary(list, request(pageToken = pageToken), CallOptions())
                },
                itemsField = list.response.findFieldByName("approvals")!!,
                nextPageTokenField = list.response.findFieldByName("next_page_token")!!,
            )
            for (page in iterator) {
                seen.addAll(page.ids())
            }
            assertEquals(listOf("apr_a", "apr_b"), seen, "the ids before the repeated token")
            assertEquals(2, transport.requests.size, "requests (it stopped on the repeated token)")
        }

        Harness.test("$NAME the two field names come off the schema, not off this test") {
            // Which fields it pages on is asked of the **response descriptor** and
            // the **request descriptor**, so a proto that renames `approvals` or
            // `next_page_token` changes the iterator rather than a list in a test.
            assertTrue(
                list.response.findFieldByName("approvals") != null,
                "ListApprovalsResponse to declare a repeated `approvals`",
            )
            assertTrue(
                list.response.findFieldByName("next_page_token") != null,
                "ListApprovalsResponse to declare `next_page_token`",
            )
            assertTrue(list.request.findFieldByName("page_size") != null, "ListApprovalsRequest to declare `page_size`")
            assertTrue(list.request.findFieldByName("page_token") != null, "ListApprovalsRequest to declare `page_token`")
            // And `FacadeOptions.pagination` declares none of them yet, so the
            // binding carries no pagination and the iterator is not on a facade path.
            assertTrue(
                Descriptors.bindingForRpc("loams.approvals.v1.ApprovalService/ListApprovals")!!.pagination == null,
                "no FacadeOptions.pagination annotation on any RPC yet",
            )
        }

        Harness.test("$NAME no RPC carries a consistency_token, so the session is inert") {
            // R4 is pinned against a stub, deliberately: a silently-merged token
            // reads stale data, which is worse than a failure. Nothing to merge
            // means the session store must not invent a token and must not claim it
            // saw one.
            val session = dev.loams.ConsistencySession()
            val getInstance = StubTransport.binding("loams.instance.v1.InstanceService/GetInstance")
            assertTrue(
                getInstance.response.findFieldByName("consistency_token") == null,
                "GetInstanceResponse to carry no consistency_token (R4's note says none does)",
            )
            assertTrue(
                session.merge("a-token") == null,
                "a merge of a token no RPC returned to report nothing to merge",
            )
            assertTrue(session.token() == null, "no token to attach to later reads")
        }

        Harness.test("kotlin_the_pagination_iterator_sends_one_call_per_page") { paginationSendsOneCallPerPage() }
        Harness.test("kotlin_pagination_refuses_an_unknown_items_field") { paginationRefusesAnUnknownItemsField() }
        Harness.test("kotlin_a_repeated_token_does_not_re_ask_forever") { aRepeatedTokenTerminates() }
    }
}

/** Reads the `id` of every approval on a page, for the assertions above. */
private fun DynamicMessage.ids(): List<String> {
    val approvals = descriptorForType.findFieldByName("approvals")
    if (approvals != null) {
        @Suppress("UNCHECKED_CAST")
        val items = getField(approvals) as List<DynamicMessage>
        return items.map { it.getField(it.descriptorForType.findFieldByName("id")).toString() }
    }
    val id = descriptorForType.findFieldByName("id")
    if (id != null) {
        return listOf(getField(id).toString())
    }
    return emptyList()
}

/**
 * `kotlin_the_pagination_iterator_sends_one_call_per_page` — and sends the token
 * the server gave, in the field the schema names.
 */
fun paginationSendsOneCallPerPage() {
    val list = PaginationTest.list
    val descriptor = list.response
    val approvals = descriptor.findFieldByName("approvals")!!
    val token = descriptor.findFieldByName("next_page_token")!!
    val approvalDescriptor = approvals.messageType

    val transport = StubTransport()
    transport.answerMessage(list.response, PaginationTest.response(listOf("apr_a"), "t2"))
    transport.answerMessage(list.response, PaginationTest.response(listOf("apr_b"), ""))

    val pages = PageIterator(
        fetch = { pageToken ->
            val request = DynamicMessage.newBuilder(list.request)
                .setField(list.request.findFieldByName("page_size"), java.lang.Integer.valueOf(50))
                .setField(list.request.findFieldByName("page_token"), pageToken)
                .build()
            transport.client().invoker.unary(list, request, CallOptions())
        },
        itemsField = approvals,
        nextPageTokenField = token,
    ).toList()

    assertEquals(2, pages.size, "pages")
    assertEquals(ContentTypes.CONNECT_UNARY_PROTO, transport.requests.first().contentType, "a Connect unary's content type")
    assertEquals(approvalDescriptor.fullName, approvals.messageType.fullName, "the items field's type")
}

/** A guard that a message with no items field is refused at construction, not later. */
fun paginationRefusesAnUnknownItemsField() {
    val list = PaginationTest.list
    assertThrows<IllegalArgumentException>("an items field the response does not declare") {
        PageIterator<DynamicMessage>(
            fetch = { DynamicMessage.getDefaultInstance(list.response) },
            itemsField = list.response.findFieldByName("no_such_field")
                ?: throw IllegalArgumentException("no_such_field"),
            nextPageTokenField = list.response.findFieldByName("next_page_token")!!,
        )
    }
    assertThrows<IllegalArgumentException>("a next_page_token field the response does not declare") {
        PageIterator<DynamicMessage>(
            fetch = { DynamicMessage.getDefaultInstance(list.response) },
            itemsField = list.response.findFieldByName("approvals")!!,
            nextPageTokenField = list.response.findFieldByName("no_such_field")
                ?: throw IllegalArgumentException("no_such_field"),
        )
    }
}

/**
 * `kotlin_a_truncated_stream_does_not_re_ask_forever` — the iterator's stop
 * condition is a **repeated token**, checked before the next fetch.
 */
fun aRepeatedTokenTerminates() {
    val list = PaginationTest.list
    val approvals = list.response.findFieldByName("approvals")!!
    val token = list.response.findFieldByName("next_page_token")!!

    var calls = 0
    val pages = PageIterator(
        fetch = {
            calls++
            DynamicMessage.newBuilder(list.response)
                .addRepeatedField(
                    approvals,
                    DynamicMessage.newBuilder(approvals.messageType)
                        .setField(approvals.messageType.findFieldByName("id"), "apr_$calls")
                        .build(),
                )
                .setField(token, "always-the-same")
                .build()
        },
        itemsField = approvals,
        nextPageTokenField = token,
    ).toList()

    assertEquals(2, pages.size, "pages before it stopped")
    assertEquals(2, calls, "fetches before it stopped")
}