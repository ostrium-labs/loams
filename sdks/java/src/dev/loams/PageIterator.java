package dev.loams;

import com.google.protobuf.Descriptors;
import com.google.protobuf.Message;
import dev.loams.facade.CallBinding;
import dev.loams.facade.Facade;
import dev.loams.facade.Pagination;
import java.util.Iterator;
import java.util.NoSuchElementException;

/**
 * Pagination (design §44 §7.4, D617; runtime contract R6).
 *
 * <p>AIP-158: {@code page_size} and {@code page_token} in, {@code next_page_token} out. The
 * generated binding says which two fields those are ({@code FacadeOptions.pagination} is
 * {@code "<items>:<next page token>"}), so the iterator is one class for every paged RPC rather
 * than one per list RPC.
 *
 * <p>In Java it is an {@link Iterator}, which is the language's own iteration shape, so a caller
 * writes:
 *
 * <pre>{@code
 * PageIterator<Collection> items = PageIterator.of(binding, client.collections()::listCollections);
 * for (Collection collection : items) {
 *     use(collection);
 * }
 * items.error(); // why the iteration stopped early, or null
 * }</pre>
 *
 * <p>The iterator-with-an-error-beside-it split is the same one {@link Stream} makes, and for the
 * same reason: an {@code Iterator} cannot report an error, so an error has to live beside it — and
 * a caller who forgets to check it sees a silently short page sequence, which for a paginated list
 * looks exactly like the end of the list.
 *
 * <p><b>No RPC is paged yet.</b> {@code loams.collection.v1.ListCollections} arrives with API1
 * Task 2, so the end-to-end half of {@code java_pagination_iterator} is a deliberate skip rather
 * than an omission: a fixture for an RPC the server does not serve would test the stub rather than
 * the SDK. What is pinned is the SDK's half — the token threading, the stop condition, and what
 * happens when a binding is not paged.
 *
 * @param <Item> the item type the paged call yields
 */
public final class PageIterator<Item> implements Iterator<Item>, Iterable<Item> {

    /**
     * Makes one page request.
     *
     * <p>A generated module method satisfies it, so the iterator drives the same code path an
     * application does rather than a parallel one.
     *
     * @param <Res> the response message type
     */
    @FunctionalInterface
    public interface PageFetcher<Res> {

        /**
         * @param pageToken the token of the page to fetch, or the empty string for the first one
         */
        Res fetch(String pageToken);
    }

    private final CallBinding binding;
    private final Pagination fields;
    private final PageFetcher<Message> fetch;

    private String nextToken = "";
    private boolean started;
    private boolean exhausted;

    /**
     * That {@link #hasNext()} has already established there is an item ready to take.
     *
     * <p>It exists because {@link #hasNext()} here is <em>not</em> side-effect-free — it fetches
     * pages — and an {@link java.util.Iterator} whose {@code next()} re-ran it would skip an item
     * on every boundary. A caller's loop does {@code hasNext(); next();}, and a second advance
     * between the two would move {@code currentPage} past the item the first call promised: the
     * last item of each page would be dropped and the fetch would race ahead. Flagging the
     * established item is what makes the pair behave.
     */
    private boolean advanced;
    private LoamsException error;
    private java.util.Iterator<?> currentPage = java.util.Collections.emptyIterator();
    private final boolean valid;

    private PageIterator(
            CallBinding binding, Pagination fields, PageFetcher<Message> fetch, boolean valid) {
        this.binding = binding;
        this.fields = fields;
        this.fetch = fetch;
        this.valid = valid;
    }

    /**
     * An iterator over every item of a paged call, following the tokens to the end (D617's "the
     * paging iterator").
     *
     * @param binding the generated binding, which names the four fields
     * @param fetch the page call
     * @param <Item> the item type
     */
    @SuppressWarnings("unchecked")
    public static <Item> PageIterator<Item> of(
            CallBinding binding, PageFetcher<?> fetch) {
        Pagination fields = binding.pagination();
        if (fields == null) {
            // Reported through `error()` rather than thrown at the call: the iterator is a value
            // the caller holds, and a factory that throws makes the common case an error path.
            // The message names the binding so the cause is obvious in a log.
            PageIterator<Item> refused =
                    new PageIterator<>(
                            binding, null, request -> null, false);
            refused.error =
                    Errors.internal(
                            binding.rpc(),
                            binding.module()
                                    + "."
                                    + binding.name()
                                    + " is not a paged call: the proto's facade options name no"
                                    + " pagination",
                            null);
            return refused;
        }
        return new PageIterator<>(binding, fields, (PageFetcher<Message>) fetch, true);
    }

    /** The iterator for a call identified by module and name, with the page call supplied. */
    public static <Item> PageIterator<Item> of(
            Client client, String module, String name, PageFetcher<?> fetch) {
        return of(bindingOf(module, name), fetch);
    }

    private static CallBinding bindingOf(String module, String name) {
        return Facade.binding(module, name)
                .orElseThrow(
                        () ->
                                Errors.internal(
                                        "", "loams." + module + " has no generated call " + name, null));
    }

    /** The request field names a paged call uses, from its binding. */
    public static Pagination pageFields(CallBinding binding) {
        return binding.pagination();
    }

    /**
     * The iterator itself, so {@code for (Item item : iterator)} works.
     *
     * <p>{@link java.util.Iterator} is the shape the language already has for this, and adding
     * {@link Iterable} costs one method rather than a new type — which is the difference between
     * a Java SDK that reads like Java and one that reads like a transliteration.
     */
    @Override
    public Iterator<Item> iterator() {
        return this;
    }

    @Override
    public boolean hasNext() {
        if (advanced) {
            // An item is already staged, so re-checking must not fetch again. See `advanced`.
            return true;
        }
        if (!valid || exhausted) {
            return false;
        }
        while (!currentPage.hasNext()) {
            // A page with no items but a next token is legal — a filter that matched nothing on
            // one shard says so rather than ending the list — so the loop keeps asking rather
            // than stopping on the first empty page.
            if (exhausted) {
                return false;
            }
            loadNextPage();
        }
        advanced = true;
        return true;
    }

    /** Fetch one page and point the inner iterator at it. */
    private void loadNextPage() {
        started = true;
        Message page;
        try {
            page = fetch.fetch(nextToken);
        } catch (RuntimeException thrown) {
            error = Errors.toLoamsException(thrown, binding.rpc());
            exhausted = true;
            return;
        }
        if (page == null) {
            exhausted = true;
            return;
        }
        // The response's own fields, read through the descriptor rather than through generated
        // getters, which is what lets one iterator serve every paged RPC.
        Descriptors.FieldDescriptor itemsField = stringOrMessageField(page, fields.itemsField());
        if (itemsField == null || !itemsField.isRepeated()) {
            error =
                    Errors.internal(
                            binding.rpc(),
                            binding.module()
                                    + "."
                                    + binding.name()
                                    + " pages on "
                                    + fields.itemsField()
                                    + ", which the response does not declare as a repeated field",
                            null);
            exhausted = true;
            return;
        }
        int count = page.getRepeatedFieldCount(itemsField);
        java.util.List<Object> items = new java.util.ArrayList<>(count);
        for (int index = 0; index < count; index++) {
            items.add(page.getRepeatedField(itemsField, index));
        }
        currentPage = items.iterator();
        nextToken = readString(page, fields.nextPageTokenField());
        if (nextToken.isEmpty()) {
            // The last page. Exhausting it ends the iteration.
            exhausted = true;
        }
    }

    @Override
    public Item next() {
        if (!hasNext()) {
            throw new NoSuchElementException("the page sequence has ended");
        }
        // Consume the staged item without letting `hasNext()` above advance a second time.
        advanced = false;
        @SuppressWarnings("unchecked")
        Item item = (Item) currentPage.next();
        return item;
    }

    /**
     * Why the iteration stopped early, or {@code null} when it reached the end.
     *
     * <p>It is {@code null} before the iteration starts, and {@code null} after it finished on the
     * last page. It is non-null when a page fetch failed, or when the binding named a field the
     * response does not have.
     */
    public LoamsException error() {
        return error;
    }

    /** Whether any page was fetched, which is how a caller tells an empty list from a refusal. */
    public boolean started() {
        return started;
    }

    /**
     * The request's page token, or the empty string for the first page.
     *
     * <p>The iterator hands it to {@link PageFetcher} rather than mutating the caller's request,
     * so the caller's message is the same object on every page.
     */
    private static Descriptors.FieldDescriptor stringOrMessageField(Message message, String name) {
        if (message == null || name == null || name.isEmpty()) {
            return null;
        }
        return message.getDescriptorForType().findFieldByName(name);
    }

    private static String readString(Message message, String name) {
        Descriptors.FieldDescriptor field = stringOrMessageField(message, name);
        if (field == null
                || field.getType() != Descriptors.FieldDescriptor.Type.STRING
                || !message.hasField(field)) {
            return "";
        }
        Object value = message.getField(field);
        return value instanceof String text ? text : "";
    }
}