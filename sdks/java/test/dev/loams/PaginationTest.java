package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import dev.loams.facade.CallBinding;
import dev.loams.facade.Pagination;
import dev.loams.gen.loams.approvals.v1.Approval;
import dev.loams.gen.loams.approvals.v1.ListApprovalsRequest;
import dev.loams.gen.loams.approvals.v1.ListApprovalsResponse;
import java.util.ArrayList;
import java.util.List;
import java.util.NoSuchElementException;
import org.junit.Test;

/**
 * SDK2 Task 6's {@code java_pagination_iterator} (design §44 §7.4, D617; runtime contract R6).
 *
 * <p>AIP-158: {@code page_size} in, {@code next_page_token} out. The generated binding names the
 * two response fields, so <b>one</b> iterator serves every paged RPC rather than one per list RPC —
 * and that is what this tests, over a <b>real generated message</b>: {@code ListApprovalsResponse}
 * declares {@code approvals}, {@code next_page_token} and {@code page_size}, and the iterator
 * reaches all of them through the same descriptor path a shipped paged call would use.
 *
 * <p><b>No generated <em>call</em> is paged yet</b> — {@code ListApprovalsResponse} exists in the
 * protos but no module method returns it, because API1 Task 2 has not wired the approvals module
 * up. So the end-to-end half of this clause is a deliberate skip rather than an omission: a
 * fixture for an RPC the SDK does not expose would test the stub rather than the SDK. What is
 * pinned is the SDK's half — the token threading, the stop condition, and what happens when a
 * binding is not paged.
 */
public class PaginationTest {

    /** The binding-shaped pagination: {@code approvals} and {@code next_page_token}, with AIP-158 names. */
    private static final Pagination PAGED =
            new Pagination("approvals", "next_page_token", "page_size", "page_token");

    /** A call binding shaped like a real paged one, built for the iterator rather than fetched. */
    private static final CallBinding PAGED_BINDING =
            new CallBinding(
                    "approvals",
                    "ListApprovals",
                    "listApprovals",
                    "ListApprovals",
                    "loams.approvals.v1.ApprovalsService/ListApprovals",
                    "loams.approvals.v1.ApprovalsService",
                    "loams.approvals.v1",
                    dev.loams.facade.IdempotencyLevel.NO_SIDE_EFFECTS,
                    dev.loams.facade.RetryClass.SAFE,
                    dev.loams.facade.Streaming.UNARY,
                    PAGED,
                    false);

    /**
     * SDK2 Task 6's {@code java_pagination_iterator}.
     *
     * <p>Pinned here: the iterator follows {@code next_page_token} to the end, it yields
     * <b>items</b> rather than pages, and it stops on an empty token rather than looping.
     */
    @Test
    public void java_pagination_iterator() {
        List<String> requestedTokens = new ArrayList<>();
        // Three pages: two items, two items, then one item with no token. The iterator has to ask
        // for all three and stop after the third.
        List<ListApprovalsResponse> pages =
                List.of(
                        page(List.of("a", "b"), "p2"),
                        page(List.of("c", "d"), "p3"),
                        page(List.of("e"), ""));

        PageIterator<Approval> items =
                PageIterator.of(
                        PAGED_BINDING,
                        token -> {
                            requestedTokens.add(token);
                            return pages.get(requestedTokens.size() - 1);
                        });

        List<String> seen = new ArrayList<>();
        for (Approval approval : items) {
            seen.add(approval.getId());
        }

        assertEquals(
                "the iterator yielded "
                        + seen
                        + ", want every item across every page in order",
                List.of("a", "b", "c", "d", "e"),
                seen);
        assertEquals(
                "the iterator asked for "
                        + requestedTokens
                        + "; the first page must be asked for with no token and each later one with"
                        + " the previous next_page_token",
                List.of("", "p2", "p3"),
                requestedTokens);
        assertNull("the iteration ended in a failure", items.error());
        assertTrue("no page was ever fetched", items.started());

        // And the iterator is exhausted rather than restarting: asking again changes nothing.
        assertFalse("the iterator kept yielding after the last page", items.hasNext());
    }

    /**
     * The field names in the binding are the ones the generated response actually declares (R6).
     *
     * <p>Without this the iterator could pass its own tests against a message it invented and
     * still fail on the first real paged response, because the field names are strings.
     */
    @Test
    public void theBindingNamesFieldsTheGeneratedResponseDeclares() {
        ListApprovalsResponse response = page(List.of("a"), "");
        assertNotNull(
                "the binding's items field \""
                        + PAGED.itemsField()
                        + "\" is not a field of ListApprovalsResponse",
                response.getDescriptorForType().findFieldByName(PAGED.itemsField()));
        assertTrue(
                "the items field is not repeated, so the iterator could never walk it",
                response.getDescriptorForType().findFieldByName(PAGED.itemsField()).isRepeated());
        assertNotNull(
                "the binding's token field \""
                        + PAGED.nextPageTokenField()
                        + "\" is not a field of ListApprovalsResponse",
                response.getDescriptorForType().findFieldByName(PAGED.nextPageTokenField()));
        // And the request side, which the page call is expected to set.
        assertNotNull(
                "the binding's page_size field is not a field of ListApprovalsRequest",
                ListApprovalsRequest.getDefaultInstance()
                        .getDescriptorForType()
                        .findFieldByName(PAGED.pageSizeFieldOrDefault()));
        assertNotNull(
                "the binding's page_token field is not a field of ListApprovalsRequest",
                ListApprovalsRequest.getDefaultInstance()
                        .getDescriptorForType()
                        .findFieldByName(PAGED.pageTokenFieldOrDefault()));
    }

    /**
     * An empty page with a token does not end the iteration (R6).
     *
     * <p>A filter that matched nothing on one shard legitimately returns an empty page and still
     * has more to say, so stopping on the first empty page would silently truncate the list —
     * which for a paginated read looks exactly like the end of it.
     */
    @Test
    public void anEmptyPageWithATokenDoesNotEndTheIteration() {
        List<ListApprovalsResponse> pages =
                List.of(page(List.of(), "p2"), page(List.of("only"), ""));
        List<String> requestedTokens = new ArrayList<>();

        PageIterator<Approval> items =
                PageIterator.of(
                        PAGED_BINDING,
                        token -> {
                            requestedTokens.add(token);
                            return pages.get(requestedTokens.size() - 1);
                        });

        List<String> seen = new ArrayList<>();
        for (Approval approval : items) {
            seen.add(approval.getId());
        }
        assertEquals(
                "the iteration stopped on the empty first page and lost the rest",
                List.of("only"),
                seen);
        assertEquals(List.of("", "p2"), requestedTokens);
    }

    /**
     * A page fetch that fails is reported through {@code error()} and stops the iteration (R6).
     *
     * <p>An {@link java.util.Iterator} cannot report an error, so it has to live beside it — and a
     * caller who forgets to check it sees a silently short page sequence, which is
     * indistinguishable from the end of the list.
     */
    @Test
    public void aFailedPageIsReportedAndStopsTheIteration() {
        List<String> requestedTokens = new ArrayList<>();
        PageIterator<Approval> items =
                PageIterator.of(
                        PAGED_BINDING,
                        token -> {
                            requestedTokens.add(token);
                            if (requestedTokens.size() == 1) {
                                return page(List.of("a"), "p2");
                            }
                            throw Errors.internal(PAGED_BINDING.rpc(), "the second page failed", null);
                        });

        List<String> seen = new ArrayList<>();
        while (items.hasNext()) {
            seen.add(items.next().getId());
        }
        assertEquals("the first page's items were lost", List.of("a"), seen);
        assertNotNull("a failed page was swallowed", items.error());
        assertEquals(
                "the failure lost its reason",
                dev.loams.facade.Reason.INTERNAL,
                items.error().reason());
    }

    /**
     * A binding that does not page reports why, through {@code error()} rather than by throwing.
     *
     * <p>The message names the binding so the cause is obvious in a log, and it is reported rather
     * than thrown because the iterator is a value the caller holds.
     */
    @Test
    public void aBindingThatDoesNotPageSaysSo() {
        // `GetInstance` is a real call in the binding table and is not paged, which is the honest
        // stand-in for the RPC that will be.
        CallBinding notPaged = Client.binding("instance", "GetInstance");
        assertNull("GetInstance claims to be paged", notPaged.pagination());

        PageIterator<Object> items = PageIterator.of(notPaged, token -> null);
        assertFalse("a non-paged call produced items", items.hasNext());
        assertNotNull("a non-paged call did not say why it produced nothing", items.error());
        assertTrue(
                "the message does not name the call: " + items.error().getMessage(),
                items.error().getMessage().contains("instance.GetInstance"));
        assertTrue(
                "the message does not explain the cause: " + items.error().getMessage(),
                items.error().getMessage().contains("no pagination"));
    }

    /**
     * A binding naming a field the response does not have is reported, not silently empty.
     *
     * <p>Returning nothing would be indistinguishable from an empty list, which is the failure
     * mode R6 is about.
     */
    @Test
    public void aFieldTheResponseDoesNotDeclareIsReported() {
        Pagination wrong = new Pagination("collections", "next_page_token", "page_size", "page_token");
        CallBinding binding =
                new CallBinding(
                        "approvals",
                        "ListApprovals",
                        "listApprovals",
                        "ListApprovals",
                        PAGED_BINDING.rpc(),
                        PAGED_BINDING.service(),
                        PAGED_BINDING.protoPackage(),
                        dev.loams.facade.IdempotencyLevel.NO_SIDE_EFFECTS,
                        dev.loams.facade.RetryClass.SAFE,
                        dev.loams.facade.Streaming.UNARY,
                        wrong,
                        false);

        PageIterator<Object> items =
                PageIterator.of(binding, token -> ListApprovalsResponse.getDefaultInstance());
        assertFalse("a misnamed items field produced items", items.hasNext());
        assertNotNull(
                "a misnamed items field was silently treated as an empty page", items.error());
        assertTrue(
                "the message does not name the field: " + items.error().getMessage(),
                items.error().getMessage().contains("collections"));
    }

    /**
     * {@code next()} past the end throws rather than returning {@code null} (R6).
     *
     * <p>The {@link java.util.Iterator} contract requires {@link NoSuchElementException}, and
     * returning {@code null} would turn a caller's loop into a silent {@code null} item it passes
     * on.
     */
    @Test(expected = NoSuchElementException.class)
    public void nextPastTheEndThrows() {
        PageIterator<Approval> items =
                PageIterator.of(PAGED_BINDING, token -> page(List.of("a"), ""));
        assertTrue(items.hasNext());
        items.next();
        items.next();
    }

    /**
     * The token is handed to the page call rather than written into the caller's request (R6).
     *
     * <p>Writing it into a message would mean copying the request and then handing the caller's own
     * message back with a page token on it, which is a mutation the caller did not ask for.
     */
    @Test
    public void pagingDoesNotMutateTheCallersRequest() {
        ListApprovalsRequest request =
                ListApprovalsRequest.getDefaultInstance().toBuilder().setPageSize(50).build();
        ListApprovalsRequest asSent = request;

        PageIterator<Approval> items =
                PageIterator.of(
                        PAGED_BINDING,
                        token -> {
                            // The page call would set the token on a copy. This asserts the
                            // iterator never had a message to touch in the first place: the token
                            // arrives as an argument.
                            if (!token.isEmpty()) {
                                assertEquals(
                                        "the iterator mutated the caller's request's token",
                                        "",
                                        asSent.getPageToken());
                            }
                            return page(List.of("a"), "");
                        });
        items.forEachRemaining(approval -> {});
        assertEquals("the page size was changed", 50, asSent.getPageSize());
        assertEquals("a token was written into the caller's request", "", asSent.getPageToken());
    }

    /**
     * The AIP-158 field names come from the binding, with the defaults filled in (R6).
     *
     * <p>A binding that names no specific field still pages on {@code page_size} and
     * {@code page_token}, which is what makes one iterator work for a proto that uses the defaults.
     */
    @Test
    public void theAip158DefaultsAreFilledIn() {
        Pagination bare = new Pagination("approvals", "next_page_token", "", "");
        assertEquals("page_size", bare.pageSizeFieldOrDefault());
        assertEquals("page_token", bare.pageTokenFieldOrDefault());
        assertEquals("page_size", PAGED.pageSizeFieldOrDefault());
        assertEquals("page_token", PAGED.pageTokenFieldOrDefault());
        assertEquals(
                "the binding's field names are not what the iterator reads",
                PAGED,
                PageIterator.pageFields(PAGED_BINDING));
    }

    /**
     * No generated call is paged yet, and that is checked rather than assumed.
     *
     * <p>When API1 Task 2 lands {@code ListApprovals} as a module method, this test starts
     * failing — which is the point. It is a reminder that the end-to-end half of
     * {@link #java_pagination_iterator()} is still owed.
     */
    @Test
    public void noGeneratedCallIsPagedYet() {
        for (var module : dev.loams.facade.Facade.MODULES) {
            for (CallBinding call : module.calls()) {
                assertNull(
                        "binding "
                                + call.module()
                                + "."
                                + call.name()
                                + " is paged. API1 Task 2 has landed, so the end-to-end half of"
                                + " java_pagination_iterator is owed a real RPC and this reminder"
                                + " should be updated with it",
                        call.pagination());
            }
        }
    }

    /** One page of {@code ListApprovalsResponse}, shaped the way a paged response would be. */
    private static ListApprovalsResponse page(List<String> ids, String nextPageToken) {
        ListApprovalsResponse.Builder builder = ListApprovalsResponse.newBuilder();
        for (String id : ids) {
            builder.addApprovals(Approval.newBuilder().setId(id).build());
        }
        if (nextPageToken != null && !nextPageToken.isEmpty()) {
            builder.setNextPageToken(nextPageToken);
        }
        return builder.build();
    }
}