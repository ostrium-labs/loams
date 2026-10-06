// Pagination: `page_size` in, `next_page_token` out, an iterator of items (design
// §44 §7.4; runtime contract R6).
//
// AIP-158, and one function serves every paged RPC because the generated binding
// names the two fields. A per-call alias (`listAll`) appears when there is a
// generated signature to hang it on, which `ListCollections` brings with API1 Task
// 2 — until then `client.PaginateAsync` is the whole of it, and nothing in this
// SDK offers a method that pretends a paged RPC exists.
//
// # What is deliberately absent
//
// The end-to-end half of this clause is **not** implemented against a live RPC,
// because no RPC is paged yet: `ListApprovals` declares `page_size` and answers
// `next_page_token`, and the mock honours neither — which is what
// `mock_status_list_is_not_paged` records, and it is why the manifest marks R6's
// three fixtures `required: false`. So the iterator is exercised against a stub in
// `csharp_pagination_iterator`, and the test asserts that no annotated RPC is paged
// yet, so the day one is, the test fails and says what is owed rather than the gap
// becoming permanent.

using System.Collections.Frozen;
using System.Reflection;

namespace Loams;

/// <summary>One page's worth of a paged call, for a caller who wants pages rather than items.</summary>
/// <typeparam name="TResponse">The page message type.</typeparam>
/// <typeparam name="TItem">The item type.</typeparam>
/// <param name="Items">The page's items, in the order the server sent them.</param>
/// <param name="NextPageToken">
/// The token for the next page, or the empty string when this was the last one. The
/// **server's** answer, never a guess: a client that invented a token would either
/// loop or stop early, and both look like a complete result.
/// </param>
/// <param name="Response">The page message itself, for a caller who wants more than the items.</param>
public sealed record Page<TResponse, TItem>(IReadOnlyList<TItem> Items, string NextPageToken, TResponse Response)
    where TResponse : class;

/// <summary>Fetches one page, given the page token the iterator has reached.</summary>
/// <typeparam name="TRequest">The request message type.</typeparam>
/// <typeparam name="TResponse">The page message type.</typeparam>
public delegate Task<TResponse> PageFetcher<TRequest, TResponse>(TRequest request, string pageToken, CancellationToken cancellationToken)
    where TRequest : class;

/// <summary>The iterator: pages in, items out.</summary>
public static class Paginator
{
    /// <summary>
    /// The items of a paged call, following the tokens to the end.
    /// </summary>
    /// <typeparam name="TRequest">The request message type.</typeparam>
    /// <typeparam name="TResponse">The page message type.</typeparam>
    /// <typeparam name="TItem">The item type.</typeparam>
    /// <param name="binding">The call. It must declare <see cref="CallBinding.Pagination"/>.</param>
    /// <param name="fetch">One page fetch.</param>
    /// <param name="request">
    /// The first request. Its <c>PageSize</c> is honoured if the caller set one, and
    /// its <c>PageToken</c> is <b>overwritten</b> by the iterator from the first page
    /// onwards: a caller who put a token in the request has asked to start there, and
    /// honouring it is how a resumed iteration works.
    /// </param>
    /// <param name="items">Reads the items out of a page.</param>
    /// <param name="cancellationToken">The caller's token.</param>
    /// <exception cref="LoamsError">
    /// The binding is not paged. Named with the call, because a log line reading
    /// "not paged" without the binding is a debugging session.
    /// </exception>
    public static async IAsyncEnumerable<TItem> ItemsAsync<TRequest, TResponse, TItem>(
        CallBinding binding,
        PageFetcher<TRequest, TResponse> fetch,
        TRequest request,
        Func<TResponse, IReadOnlyList<TItem>> items,
        [System.Runtime.CompilerServices.EnumeratorCancellation] CancellationToken cancellationToken = default)
        where TRequest : class
        where TResponse : class
    {
        ArgumentNullException.ThrowIfNull(binding);
        ArgumentNullException.ThrowIfNull(fetch);
        ArgumentNullException.ThrowIfNull(request);
        ArgumentNullException.ThrowIfNull(items);

        if (binding.Pagination is not { } shape)
        {
            throw ErrorMapper.Internal(binding.Rpc,
                $"{binding.Module}.{binding.Name} is not a paged call: its binding declares no " +
                "items field and no next_page_token. No annotated RPC is paged yet; ListCollections " +
                "arrives with API1 Task 2.");
        }

        var token = ReadString(request, shape.PageTokenField) ?? string.Empty;
        var pages = 0;

        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();

            var page = await fetch(request, token, cancellationToken).ConfigureAwait(false);
            foreach (var item in items(page))
            {
                yield return item;
            }

            // The server's answer, read from the field the binding names. An
            // **absent** token is the end of the iteration; a token the server sent
            // and the client cannot read is an error rather than a silent stop,
            // because a client that stops there reports a complete result it does
            // not have.
            var next = ReadString(page, shape.NextPageTokenField);
            if (next is null)
            {
                throw ErrorMapper.Internal(binding.Rpc,
                    $"{binding.Module}.{binding.Name} answered a page with no readable " +
                    $"{shape.NextPageTokenField}; the iterator cannot tell whether that is the last page");
            }
            if (next.Length == 0)
            {
                yield break;
            }

            // A token that repeats is a server that is not advancing. Stopping on
            // it would loop forever; failing names the loop, which is the thing the
            // operator has to fix.
            pages++;
            if (pages > MaxPages)
            {
                throw ErrorMapper.Internal(binding.Rpc,
                    $"{binding.Module}.{binding.Name} returned more than {MaxPages} pages without " +
                    "ending, so its next_page_token is not advancing");
            }

            token = next;
            WriteString(request, shape.PageTokenField, token);
        }
    }

    /// <summary>
    /// The pages of a paged call, for a caller who wants them rather than the items.
    /// </summary>
    /// <remarks>
    /// The same walk, yielding each page instead of flattening it. It is written out
    /// rather than composed from <see cref="ItemsAsync{TRequest,TResponse,TItem}"/>
    /// because composing it would mean buffering a whole page to hand it on, which
    /// is exactly the copy a streaming iterator exists to avoid.
    /// </remarks>
    public static async IAsyncEnumerable<Page<TResponse, TItem>> PagesAsync<TRequest, TResponse, TItem>(
        CallBinding binding,
        PageFetcher<TRequest, TResponse> fetch,
        TRequest request,
        Func<TResponse, IReadOnlyList<TItem>> items,
        [System.Runtime.CompilerServices.EnumeratorCancellation] CancellationToken cancellationToken = default)
        where TRequest : class
        where TResponse : class
    {
        ArgumentNullException.ThrowIfNull(binding);
        ArgumentNullException.ThrowIfNull(fetch);

        if (binding.Pagination is not { } shape)
        {
            throw ErrorMapper.Internal(binding.Rpc,
                $"{binding.Module}.{binding.Name} is not a paged call: its binding declares no " +
                "items field and no next_page_token.");
        }

        var token = ReadString(request, shape.PageTokenField) ?? string.Empty;
        var pages = 0;

        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();

            var page = await fetch(request, token, cancellationToken).ConfigureAwait(false);
            yield return new Page<TResponse, TItem>(items(page), ReadString(page, shape.NextPageTokenField) ?? string.Empty, page);

            var next = ReadString(page, shape.NextPageTokenField);
            if (next is null)
            {
                throw ErrorMapper.Internal(binding.Rpc,
                    $"{binding.Module}.{binding.Name} answered a page with no readable " +
                    $"{shape.NextPageTokenField}; the iterator cannot tell whether that is the last page");
            }
            if (next.Length == 0)
            {
                yield break;
            }

            pages++;
            if (pages > MaxPages)
            {
                throw ErrorMapper.Internal(binding.Rpc,
                    $"{binding.Module}.{binding.Name} returned more than {MaxPages} pages without ending");
            }

            token = next;
            WriteString(request, shape.PageTokenField, token);
        }
    }

    /// <summary>
    /// How many pages one iteration may walk before it calls a non-advancing token a
    /// loop.
    /// </summary>
    /// <remarks>
    /// Not a "maximum result size": no SDK gets to decide that for a caller, and a
    /// limit here would silently truncate a large listing. It is a guard against one
    /// specific failure — a server that keeps handing back the same token — where the
    /// alternative is an iterator that never ends.
    /// </remarks>
    public const int MaxPages = 100_000;

    /// <summary>
    /// Reads a property by name, or <see langword="null"/> when it is absent.
    /// </summary>
    /// <remarks>
    /// A generated protobuf message's properties are the field names in PascalCase,
    /// which is exactly what <see cref="Pagination"/>'s binding names — so this is a
    /// property read and not a descriptor walk, and it is the same shape for the
    /// request and the response. A property that does not exist returns null rather
    /// than throwing, because "this binding names a field this message does not have"
    /// is a binding bug and the message names it in that case.
    /// </remarks>
    private static string? ReadString(object target, string propertyName)
    {
        var property = target.GetType().GetProperty(propertyName, BindingFlags.Public | BindingFlags.Instance);
        if (property is null)
        {
            return null;
        }
        return property.GetValue(target) as string;
    }

    private static void WriteString(object target, string propertyName, string value)
    {
        var property = target.GetType().GetProperty(propertyName, BindingFlags.Public | BindingFlags.Instance);
        if (property is null || !property.CanWrite)
        {
            return;
        }
        property.SetValue(target, value);
    }
}