// The collection handle (SDKP §2.8): `client.Collection("notes")` returns calls
// with the collection (and namespace) filled in. It holds no state of its own;
// every call delegates to the module call.

using Google.Protobuf.WellKnownTypes;
using Loams.Collection.V1;

namespace Loams;

/// <summary>One collection's calls.</summary>
public sealed class CollectionHandle
{
    private readonly LoamsClient _client;

    internal CollectionHandle(LoamsClient client, string name, string? @namespace)
    {
        _client = client;
        Name = name;
        Namespace = @namespace;
    }

    /// <summary>The collection's name or alias.</summary>
    public string Name { get; }

    /// <summary>The namespace, or null for the client's default.</summary>
    public string? Namespace { get; }

    public Task<WriteDocumentsResponse> UpsertAsync(IEnumerable<Doc> docs, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _client.Documents.UpsertAsync(Name, docs, Namespace, options, cancellationToken);

    public Task<GetDocumentsResponse> GetAsync(IEnumerable<DocumentId> ids, Loams.Collection.V1.Consistency? consistency = null,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        _client.Documents.GetAsync(Name, ids, consistency, Namespace, options, cancellationToken);

    public Task<WriteDocumentsResponse> DeleteAsync(IEnumerable<DocumentId> ids, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _client.Documents.DeleteAsync(Name, ids, Namespace, options, cancellationToken);

    public Task<WriteDocumentsResponse> PatchAsync(DocumentId id, object? set = null, IEnumerable<string>? unset = null,
        string? mode = null, CallOptions? options = null, CancellationToken cancellationToken = default) =>
        _client.Documents.PatchAsync(Name, id, set, unset, mode, Namespace, options, cancellationToken);

    public Task<BatchResult> WriteBatchAsync(IEnumerable<Doc> docs, int chunkRows = WriteBatch.DefaultChunkRows,
        int chunkBytes = WriteBatch.DefaultChunkBytes, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _client.Documents.WriteBatchAsync(Name, docs, Namespace, chunkRows, chunkBytes, options, cancellationToken);

    public Task<ulong> CountAsync(Query? where = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _client.Documents.CountAsync(Name, where, Namespace, options, cancellationToken);

    public IAsyncEnumerable<Document> ScrollAllAsync(Query? where = null, uint? limit = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _client.Documents.ScrollAllAsync(Name, where, limit, Namespace, options, cancellationToken);

    /// <summary>A search over this collection; the builder's collection and namespace are replaced by the handle's.</summary>
    public Task<SearchResponse> SearchAsync(Search search, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _client.Search.QueryAsync(Scoped(search.Build()), options, cancellationToken);

    /// <summary>A search over this collection, starting from an empty builder.</summary>
    public Task<SearchResponse> SearchAsync(Func<Search, Search> build, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        SearchAsync(build(Search.On(Name)), options, cancellationToken);

    public Task<SearchResponse> VectorSearchAsync(IEnumerable<float> vector, string field, uint? limit = null,
        Query? filter = null, CallOptions? options = null, CancellationToken cancellationToken = default) =>
        _client.Vector.SearchAsync(Name, vector, field, limit, filter, Namespace, options, cancellationToken);

    public Task<DropCollectionResponse> DropAsync(CallOptions? options = null, CancellationToken cancellationToken = default) =>
        _client.Collections.DropAsync(Name, Namespace, options, cancellationToken);

    /// <summary>The collection's info (<c>collections.get</c>).</summary>
    public Task<CollectionInfo> InfoAsync(CallOptions? options = null, CancellationToken cancellationToken = default) =>
        _client.Collections.GetAsync(Name, Namespace, options, cancellationToken);

    private SearchRequest Scoped(SearchRequest request)
    {
        request.Collection = Name;
        if (Namespace is not null)
        {
            request.Namespace = Namespace;
        }
        return request;
    }
}
