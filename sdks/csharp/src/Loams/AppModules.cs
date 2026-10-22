// The app-building modules (SDKP §2.2–§2.5): namespaces, collections, documents,
// search, vector and graph. Hand-written under SDKP-4; every method resolves its
// binding by name from `Facade` and names no RPC path or retry class.
//
// Every request-typed method has a shorthand beside it that takes the fields a
// caller writes most. A `namespace` argument left null takes the client's default
// (R11); `CallOptions` and the `CancellationToken` come last, as everywhere.

using System.Runtime.CompilerServices;
using Google.Protobuf.WellKnownTypes;
using Loams.Collection.V1;
using Loams.Graph.V1;
using CollectionConsistency = Loams.Collection.V1.Consistency;
using GraphConsistency = Loams.Graph.V1.Consistency;
using GraphValue = Loams.Graph.V1.Value;

namespace Loams;

/// <summary><c>loams.namespaces</c> — the container every collection and stream lives in.</summary>
public sealed class NamespacesModule
{
    private readonly CallInvoker _invoker;

    internal NamespacesModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>Creates a namespace. A taken name is <c>already_exists</c>. Retry-safe.</summary>
    public Task<CreateNamespaceResponse> CreateAsync(CreateNamespaceRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<CreateNamespaceResponse>(Facade.Binding("namespaces", "Create"), request, options,
            cancellationToken);

    /// <summary>Creates the namespace <paramref name="name"/>.</summary>
    public Task<CreateNamespaceResponse> CreateAsync(string name, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        CreateAsync(new CreateNamespaceRequest { Namespace = name }, options, cancellationToken);

    /// <summary>One page of namespaces.</summary>
    public Task<ListNamespacesResponse> ListAsync(ListNamespacesRequest? request = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<ListNamespacesResponse>(Facade.Binding("namespaces", "List"),
            request ?? new ListNamespacesRequest(), options, cancellationToken);

    /// <summary>Every namespace, following the page tokens.</summary>
    public IAsyncEnumerable<NamespaceInfo> ListAllAsync(int pageSize = 0, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Paginator.ItemsAsync<ListNamespacesRequest, ListNamespacesResponse, NamespaceInfo>(
            Facade.Binding("namespaces", "List"),
            (request, token, ct) => ListAsync(Page(request, token), options, ct),
            new ListNamespacesRequest { PageSize = pageSize }, (page) => page.Namespaces, cancellationToken);

    private static ListNamespacesRequest Page(ListNamespacesRequest request, string token)
    {
        var copy = request.Clone();
        copy.PageToken = token;
        return copy;
    }
}

/// <summary><c>loams.collections</c> — collections and their schemas.</summary>
public sealed class CollectionsModule
{
    private readonly CallInvoker _invoker;

    internal CollectionsModule(CallInvoker invoker) => _invoker = invoker;

    private Task<T> Call<T>(string name, Google.Protobuf.IMessage request, CallOptions? options, CancellationToken ct)
        where T : Google.Protobuf.IMessage<T>, new() =>
        _invoker.UnaryAsync<T>(Facade.Binding("collections", name), request, options, ct);

    /// <summary>Creates a collection. Idempotent by name; a different schema under a taken name is <c>already_exists</c>.</summary>
    public Task<CollectionInfo> CreateAsync(CreateCollectionRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) => Call<CollectionInfo>("Create", request, options, cancellationToken);

    /// <summary>Creates <paramref name="name"/> with a schema from <see cref="Schema"/>.</summary>
    public Task<CollectionInfo> CreateAsync(string name, Struct schema, uint? partitions = null, string? @namespace = null,
        CallOptions? options = null, CancellationToken cancellationToken = default)
    {
        var request = new CreateCollectionRequest { Name = name, Schema = schema, Namespace = @namespace ?? string.Empty };
        if (partitions is { } p)
        {
            request.Partitions = p;
        }
        return CreateAsync(request, options, cancellationToken);
    }

    /// <summary>One page of the namespace's collections.</summary>
    public Task<ListCollectionsResponse> ListAsync(ListCollectionsRequest? request = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<ListCollectionsResponse>("List", request ?? new ListCollectionsRequest(), options, cancellationToken);

    /// <summary>Every collection in the namespace, following the page tokens.</summary>
    public IAsyncEnumerable<CollectionInfo> ListAllAsync(string? @namespace = null, int pageSize = 0,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        Paginator.ItemsAsync<ListCollectionsRequest, ListCollectionsResponse, CollectionInfo>(
            Facade.Binding("collections", "List"),
            (request, token, ct) =>
            {
                var copy = request.Clone();
                copy.PageToken = token;
                return ListAsync(copy, options, ct);
            },
            new ListCollectionsRequest { Namespace = @namespace ?? string.Empty, PageSize = pageSize },
            (page) => page.Collections, cancellationToken);

    /// <summary>One collection, by name or alias.</summary>
    public Task<CollectionInfo> GetAsync(GetCollectionRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) => Call<CollectionInfo>("Get", request, options, cancellationToken);

    /// <summary>One collection, by name or alias.</summary>
    public Task<CollectionInfo> GetAsync(string collection, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        GetAsync(new GetCollectionRequest { Collection = collection, Namespace = @namespace ?? string.Empty }, options,
            cancellationToken);

    /// <summary>Drops a collection.</summary>
    public Task<DropCollectionResponse> DropAsync(DropCollectionRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<DropCollectionResponse>("Drop", request, options, cancellationToken);

    /// <summary>Drops a collection.</summary>
    public Task<DropCollectionResponse> DropAsync(string collection, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        DropAsync(new DropCollectionRequest { Collection = collection, Namespace = @namespace ?? string.Empty }, options,
            cancellationToken);

    /// <summary>Adds fields and vectors to a collection's schema (additive).</summary>
    public Task<AddFieldsResponse> AddFieldsAsync(AddFieldsRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<AddFieldsResponse>("AddFields", request, options, cancellationToken);

    /// <summary>One page of the collection's manifest versions.</summary>
    public Task<ListVersionsResponse> ListVersionsAsync(ListVersionsRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<ListVersionsResponse>("ListVersions", request, options, cancellationToken);

    /// <summary>Creates and deletes aliases, atomically.</summary>
    public Task<UpdateAliasesResponse> UpdateAliasesAsync(UpdateAliasesRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<UpdateAliasesResponse>("UpdateAliases", request, options, cancellationToken);

    /// <summary>A scan plan over the collection's files. Never retried.</summary>
    public Task<ScanPlan> ScanAsync(ScanRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) => Call<ScanPlan>("Scan", request, options, cancellationToken);

    /// <summary>Sets the collection's hot-tier configuration.</summary>
    public Task<SetHotResponse> SetHotAsync(SetHotRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) => Call<SetHotResponse>("SetHot", request, options, cancellationToken);

    /// <summary>Warms the collection's hot tier.</summary>
    public Task<WarmCollectionResponse> WarmAsync(WarmCollectionRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<WarmCollectionResponse>("Warm", request, options, cancellationToken);
}

/// <summary>One document to upsert: an id, its source, and its vectors.</summary>
/// <param name="Id">A number, a string or a UUID (see <see cref="DocumentId"/>'s conversions).</param>
/// <param name="Source">The source: a <see cref="Struct"/>, a dictionary, or any object (its public properties).</param>
/// <param name="Vectors">Dense vectors by name.</param>
public sealed record Doc(DocumentId Id, object Source, IReadOnlyDictionary<string, float[]>? Vectors = null)
{
    /// <summary>The write op that upserts this document.</summary>
    public WriteOp ToOp()
    {
        var doc = new WriteDocument { Id = Id, Source = Values.StructFrom(Source) };
        if (Vectors is not null)
        {
            foreach (var (name, vector) in Vectors)
            {
                doc.Vectors[name] = Values.Of(vector);
            }
        }
        return new WriteOp { Upsert = doc };
    }
}

/// <summary><c>loams.documents</c> — write, read, scroll and count documents.</summary>
public sealed class DocumentsModule
{
    private readonly CallInvoker _invoker;

    internal DocumentsModule(CallInvoker invoker) => _invoker = invoker;

    private Task<T> Call<T>(string name, Google.Protobuf.IMessage request, CallOptions? options, CancellationToken ct)
        where T : Google.Protobuf.IMessage<T>, new() =>
        _invoker.UnaryAsync<T>(Facade.Binding("documents", name), request, options, ct);

    /// <summary>Raw write ops, as one write: keyed with an idempotency key, reused on every retry (R3). Returns the write's token.</summary>
    public Task<WriteDocumentsResponse> WriteAsync(WriteDocumentsRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<WriteDocumentsResponse>("Write", request, options, cancellationToken);

    /// <summary>Upserts documents, as one write.</summary>
    public Task<WriteDocumentsResponse> UpsertAsync(string collection, IEnumerable<Doc> docs, string? @namespace = null,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        WriteAsync(Request(collection, @namespace, docs.Select((d) => d.ToOp())), options, cancellationToken);

    /// <summary>Deletes documents by id, as one write.</summary>
    public Task<WriteDocumentsResponse> DeleteAsync(string collection, IEnumerable<DocumentId> ids,
        string? @namespace = null, CallOptions? options = null, CancellationToken cancellationToken = default) =>
        WriteAsync(Request(collection, @namespace,
            ids.Select((id) => new WriteOp { Delete = new DeleteDocument { Id = id } })), options, cancellationToken);

    /// <summary>
    /// Changes one document: <paramref name="set"/> merges into its source (deep by
    /// default) and <paramref name="unset"/> removes keys.
    /// </summary>
    public Task<WriteDocumentsResponse> PatchAsync(string collection, DocumentId id, object? set = null,
        IEnumerable<string>? unset = null, string? mode = null, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var patch = new Patch { Id = id, Mode = mode ?? string.Empty };
        if (set is not null)
        {
            patch.Source = Values.StructFrom(set);
        }
        if (unset is not null)
        {
            patch.DeleteKeys.AddRange(unset);
        }
        return WriteAsync(Request(collection, @namespace, [new WriteOp { Patch = patch }]), options, cancellationToken);
    }

    /// <summary>
    /// Writes any number of ops in chunks, one chunk in flight at a time (R13). See
    /// <see cref="WriteBatch"/>.
    /// </summary>
    public Task<BatchResult> WriteBatchAsync(string collection, IEnumerable<WriteOp> ops, string? @namespace = null,
        int chunkRows = WriteBatch.DefaultChunkRows, int chunkBytes = WriteBatch.DefaultChunkBytes,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        WriteBatch.RunAsync(ops, chunkRows, chunkBytes,
            (chunk, key, ct) => WriteAsync(Request(collection, @namespace, chunk),
                (options ?? CallOptions.None) with { IdempotencyKey = key }, ct),
            cancellationToken);

    /// <summary>Upserts any number of documents in chunks (R13).</summary>
    public Task<BatchResult> WriteBatchAsync(string collection, IEnumerable<Doc> docs, string? @namespace = null,
        int chunkRows = WriteBatch.DefaultChunkRows, int chunkBytes = WriteBatch.DefaultChunkBytes,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        WriteBatchAsync(collection, docs.Select((d) => d.ToOp()), @namespace, chunkRows, chunkBytes, options,
            cancellationToken);

    /// <summary>Documents by id.</summary>
    public Task<GetDocumentsResponse> GetAsync(GetDocumentsRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<GetDocumentsResponse>("Get", request, options, cancellationToken);

    /// <summary>Documents by id.</summary>
    public Task<GetDocumentsResponse> GetAsync(string collection, IEnumerable<DocumentId> ids,
        CollectionConsistency? consistency = null, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var request = new GetDocumentsRequest
        {
            Collection = collection,
            Namespace = @namespace ?? string.Empty,
            Consistency = consistency,
        };
        request.Ids.AddRange(ids);
        return GetAsync(request, options, cancellationToken);
    }

    /// <summary>One page of documents in id order.</summary>
    public Task<ScrollDocumentsResponse> ScrollAsync(ScrollDocumentsRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<ScrollDocumentsResponse>("Scroll", request, options, cancellationToken);

    /// <summary>
    /// Every document matching <paramref name="where"/>, in id order: pages of
    /// <paramref name="limit"/>, following <c>next</c> until it is unset.
    /// </summary>
    public IAsyncEnumerable<Document> ScrollAllAsync(string collection, Query? where = null, uint? limit = null,
        string? @namespace = null, CallOptions? options = null, CancellationToken cancellationToken = default)
    {
        var request = new ScrollDocumentsRequest { Collection = collection, Namespace = @namespace ?? string.Empty };
        if (where is not null)
        {
            request.Filter = Filter.ToIr(where);
        }
        if (limit is { } l)
        {
            request.Limit = l;
        }
        return ScrollAllAsync(request, options, cancellationToken);
    }

    /// <summary>Every document a scroll request selects, following <c>next</c>.</summary>
    public async IAsyncEnumerable<Document> ScrollAllAsync(ScrollDocumentsRequest request, CallOptions? options = null,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        var page = request.Clone();
        for (var pages = 0; ; pages++)
        {
            var answer = await ScrollAsync(page, options, cancellationToken).ConfigureAwait(false);
            foreach (var document in answer.Documents)
            {
                yield return document;
            }
            if (answer.Next is null || pages >= Paginator.MaxPages)
            {
                yield break;
            }
            page.PageToken = answer.Next;
        }
    }

    /// <summary>How many documents match.</summary>
    public Task<CountDocumentsResponse> CountAsync(CountDocumentsRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<CountDocumentsResponse>("Count", request, options, cancellationToken);

    /// <summary>How many documents match <paramref name="where"/> (all of them when null).</summary>
    public async Task<ulong> CountAsync(string collection, Query? where = null, string? @namespace = null,
        CallOptions? options = null, CancellationToken cancellationToken = default)
    {
        var request = new CountDocumentsRequest { Collection = collection, Namespace = @namespace ?? string.Empty };
        if (where is not null)
        {
            request.Filter = Filter.ToIr(where);
        }
        var answer = await CountAsync(request, options, cancellationToken).ConfigureAwait(false);
        return answer.Count;
    }

    /// <summary>Deletes every match of a filter. Never retried: resumable through its cursor instead.</summary>
    public Task<FilterWriteResponse> DeleteByFilterAsync(DeleteByFilterRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<FilterWriteResponse>("DeleteByFilter", request, options, cancellationToken);

    /// <summary>Patches every match of a filter. Never retried: resumable through its cursor instead.</summary>
    public Task<FilterWriteResponse> PatchByFilterAsync(PatchByFilterRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<FilterWriteResponse>("PatchByFilter", request, options, cancellationToken);

    private static WriteDocumentsRequest Request(string collection, string? @namespace, IEnumerable<WriteOp> ops)
    {
        var request = new WriteDocumentsRequest { Collection = collection, Namespace = @namespace ?? string.Empty };
        request.Ops.AddRange(ops);
        return request;
    }
}

/// <summary><c>loams.search</c> — text, vector and hybrid search over one collection.</summary>
public sealed class SearchModule
{
    private readonly CallInvoker _invoker;

    internal SearchModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>One search: the full request IR. Build one with <see cref="Search"/>.</summary>
    public Task<SearchResponse> QueryAsync(SearchRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<SearchResponse>(Facade.Binding("search", "Query"), request, options, cancellationToken);

    /// <summary>One search, from the builder.</summary>
    public Task<SearchResponse> QueryAsync(Search search, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        QueryAsync(search.Build(), options, cancellationToken);

    /// <summary>A text search: a match on one field, a multi-match on several.</summary>
    public Task<SearchResponse> TextAsync(string collection, string text, string[] fields, uint? limit = null,
        Query? filter = null, CallOptions? options = null, CancellationToken cancellationToken = default)
    {
        var search = Search.On(collection).Text(text, fields);
        if (limit is { } l)
        {
            search.Limit(l);
        }
        if (filter is not null)
        {
            search.Filter(filter);
        }
        return QueryAsync(search, options, cancellationToken);
    }

    /// <summary>A hybrid search: text on <paramref name="textFields"/> and a vector on <paramref name="vectorField"/>, fused (RRF by default).</summary>
    public Task<SearchResponse> HybridAsync(string collection, string text, string[] textFields, string vectorField,
        IEnumerable<float> vector, Loams.Collection.V1.Fusion? fusion = null, uint? limit = null, Query? filter = null,
        CallOptions? options = null, CancellationToken cancellationToken = default)
    {
        var search = Search.On(collection).Text(text, textFields).Vector(vectorField, vector)
            .Fuse(fusion ?? Fusion.Rrf());
        if (limit is { } l)
        {
            search.Limit(l);
        }
        if (filter is not null)
        {
            search.Filter(filter);
        }
        return QueryAsync(search, options, cancellationToken);
    }

    /// <summary>
    /// Every hit, page after page, following <c>search_after</c> from the last
    /// hit's sort values until a page comes back short. The request must sort with
    /// a primary-key tiebreaker (<see cref="Sort.Pk"/>), or pages can skip or repeat
    /// hits; without one this raises <c>invalid_argument</c> before any call.
    /// </summary>
    public async IAsyncEnumerable<Hit> QueryAllAsync(SearchRequest request, CallOptions? options = null,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(request);
        if (!request.Sort.Any((key) => key.KeyCase == SortKey.KeyOneofCase.Pk))
        {
            throw new InvalidArgumentError(Facade.Binding("search", "Query").Rpc, Reason.InvalidArgument,
                "search_after_needs_tiebreak", new Dictionary<string, string> { ["field"] = "sort" },
                message: "QueryAllAsync pages with search_after, which needs a sort with a primary-key " +
                    "tiebreaker: add Sort.Pk() as the last sort key");
        }
        var page = request.Clone();
        var limit = page.HasLimit ? page.Limit : 10u;
        page.Limit = limit;
        for (var pages = 0; pages < Paginator.MaxPages; pages++)
        {
            var answer = await QueryAsync(page, options, cancellationToken).ConfigureAwait(false);
            foreach (var hit in answer.Hits)
            {
                yield return hit;
            }
            if (answer.Hits.Count < limit || answer.Hits.Count == 0)
            {
                yield break;
            }
            page.SearchAfter.Clear();
            page.SearchAfter.AddRange(answer.Hits[^1].SortValues);
            page.Offset = 0;
        }
    }

    /// <summary>Every hit of a builder's search; see <see cref="QueryAllAsync(SearchRequest, CallOptions?, CancellationToken)"/>.</summary>
    public IAsyncEnumerable<Hit> QueryAllAsync(Search search, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        QueryAllAsync(search.Build(), options, cancellationToken);
}

/// <summary><c>loams.vector</c> — the search RPC with a vector's shape (Q613).</summary>
public sealed class VectorModule
{
    private readonly CallInvoker _invoker;

    internal VectorModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>The nearest neighbours of <paramref name="vector"/> in <paramref name="field"/>.</summary>
    public Task<SearchResponse> SearchAsync(string collection, IEnumerable<float> vector, string field,
        uint? limit = null, Query? filter = null, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var search = Search.On(collection).Vector(field, vector);
        if (@namespace is not null)
        {
            search.Namespace(@namespace);
        }
        if (limit is { } l)
        {
            search.Limit(l);
        }
        if (filter is not null)
        {
            search.Filter(filter);
        }
        return SearchAsync(search.Build(), options, cancellationToken);
    }

    /// <summary>A vector search, as the full request.</summary>
    public Task<SearchResponse> SearchAsync(SearchRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<SearchResponse>(Facade.Binding("vector", "Search"), request, options, cancellationToken);
}

/// <summary>
/// <c>loams.graph</c> — stored property graphs, queried with GQL. <b>Experimental</b>:
/// the package is <c>unstable</c> (GR1e) and may change.
/// </summary>
public sealed class GraphModule
{
    private readonly CallInvoker _invoker;

    internal GraphModule(CallInvoker invoker) => _invoker = invoker;

    private Task<T> Call<T>(string name, Google.Protobuf.IMessage request, CallOptions? options, CancellationToken ct)
        where T : Google.Protobuf.IMessage<T>, new() =>
        _invoker.UnaryAsync<T>(Facade.Binding("graph", name), request, options, ct);

    // `update` and `delete` are never retried (SDKP §2.5), so the SDK does not mint
    // their idempotency key; a caller who supplies one makes them retryable.
    private static CallOptions Unkeyed(CallOptions? options) =>
        options?.IdempotencyKey is { Length: > 0 } ? options : (options ?? CallOptions.None) with { MintIdempotencyKey = false };

    /// <summary>Creates a graph. Keyed (R3).</summary>
    public Task<Loams.Graph.V1.Graph> CreateAsync(CreateGraphRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<Loams.Graph.V1.Graph>("Create", request, options, cancellationToken);

    /// <summary>Creates an owned graph named <paramref name="name"/>, queried with GQL.</summary>
    public Task<Loams.Graph.V1.Graph> CreateAsync(string name, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        CreateAsync(new CreateGraphRequest { Name = name, Namespace = @namespace ?? string.Empty }, options,
            cancellationToken);

    /// <summary>One graph.</summary>
    public Task<Loams.Graph.V1.Graph> GetAsync(string name, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<Loams.Graph.V1.Graph>("Get", new GetGraphRequest { Name = name, Namespace = @namespace ?? string.Empty },
            options, cancellationToken);

    /// <summary>One page of graphs.</summary>
    public Task<ListGraphsResponse> ListAsync(ListGraphsRequest? request = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<ListGraphsResponse>("List", request ?? new ListGraphsRequest(), options, cancellationToken);

    /// <summary>Updates a graph. Never retried unless the caller supplies a key.</summary>
    public Task<Loams.Graph.V1.Graph> UpdateAsync(UpdateGraphRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<Loams.Graph.V1.Graph>("Update", request, Unkeyed(options), cancellationToken);

    /// <summary>Deletes a graph; answers the deletion's operation. Never retried unless the caller supplies a key.</summary>
    public Task<Loams.Operations.V1.Operation> DeleteAsync(string name, string? @namespace = null,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        Call<Loams.Operations.V1.Operation>("Delete",
            new DeleteGraphRequest { Name = name, Namespace = @namespace ?? string.Empty }, Unkeyed(options),
            cancellationToken);

    /// <summary>The graph's labels, edge types, property keys and indexes.</summary>
    public Task<GraphSchema> SchemaAsync(string name, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<GraphSchema>("Schema", new GetSchemaRequest { Name = name, Namespace = @namespace ?? string.Empty },
            options, cancellationToken);

    /// <summary>
    /// Runs one statement. A read-only statement is retried freely; a write is
    /// keyed (R3) and retried with the same key.
    /// </summary>
    public Task<ExecuteResponse> QueryAsync(ExecuteRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var settings = options ?? CallOptions.None;
        if (request.ReadOnly)
        {
            settings = settings with { RetrySafe = settings.RetrySafe ?? true, MintIdempotencyKey = settings.MintIdempotencyKey ?? false };
        }
        return Call<ExecuteResponse>("Query", request, settings, cancellationToken);
    }

    /// <summary>Runs one GQL statement with parameters.</summary>
    public Task<ExecuteResponse> QueryAsync(string graph, string statement,
        IReadOnlyDictionary<string, object?>? parameters = null, bool readOnly = false,
        GraphConsistency? consistency = null, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var request = new ExecuteRequest
        {
            Graph = graph,
            Statement = statement,
            Language = QueryLanguage.Gql,
            ReadOnly = readOnly,
            Consistency = consistency,
            Namespace = @namespace ?? string.Empty,
        };
        if (parameters is not null)
        {
            foreach (var (name, value) in parameters)
            {
                request.Parameters[name] = GraphValues.Of(value);
            }
        }
        return QueryAsync(request, options, cancellationToken);
    }

    /// <summary>Runs several statements, atomically when asked. Keyed.</summary>
    public Task<ExecuteBatchResponse> QueryBatchAsync(ExecuteBatchRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<ExecuteBatchResponse>("QueryBatch", request, options, cancellationToken);

    /// <summary>Runs one statement and streams its rows in chunks.</summary>
    public IAsyncEnumerable<ResultChunk> QueryStreamAsync(ExecuteStreamRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.ServerStreamAsync<ResultChunk>(Facade.Binding("graph", "QueryStream"), request, options,
            cancellationToken);

    /// <summary>The plan for a statement, or its profile.</summary>
    public Task<Plan> ExplainAsync(ExplainRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) => Call<Plan>("Explain", request, options, cancellationToken);
}

/// <summary>Plain .NET values as <c>loams.graph.v1.Value</c>, for GQL parameters.</summary>
public static class GraphValues
{
    /// <summary>A graph value: null, bool, integer, float, string, bytes, float[] (a vector), a sequence or a dictionary.</summary>
    public static GraphValue Of(object? value) => value switch
    {
        null => new GraphValue { Null = NullValue.NullValue },
        GraphValue v => v,
        bool b => new GraphValue { Boolean = b },
        string s => new GraphValue { String = s },
        int or long or short or sbyte => new GraphValue { Int64 = Convert.ToInt64(value, System.Globalization.CultureInfo.InvariantCulture) },
        byte or ushort or uint => new GraphValue { Int64 = Convert.ToInt64(value, System.Globalization.CultureInfo.InvariantCulture) },
        ulong u => new GraphValue { Uint64 = u },
        double d => new GraphValue { Float64 = d },
        float f => new GraphValue { Float64 = f },
        decimal m => new GraphValue { Decimal = new Loams.Graph.V1.Decimal { Value = m.ToString(System.Globalization.CultureInfo.InvariantCulture) } },
        byte[] bytes => new GraphValue { Bytes = Google.Protobuf.ByteString.CopyFrom(bytes) },
        float[] vector => new GraphValue { Vector = new Loams.Graph.V1.Vector { Values = { vector } } },
        IEnumerable<KeyValuePair<string, object?>> map => Map(map),
        System.Collections.IEnumerable list => List(list),
        _ => throw new ArgumentException($"cannot send a {value.GetType().Name} as a graph value", nameof(value)),
    };

    private static GraphValue Map(IEnumerable<KeyValuePair<string, object?>> map)
    {
        var m = new MapValue();
        foreach (var (key, item) in map)
        {
            m.Entries[key] = Of(item);
        }
        return new GraphValue { Map = m };
    }

    private static GraphValue List(System.Collections.IEnumerable list)
    {
        var l = new Loams.Graph.V1.ListValue();
        foreach (var item in list)
        {
            l.Values.Add(Of(item));
        }
        return new GraphValue { List = l };
    }

    /// <summary>A graph value as a plain .NET value.</summary>
    public static object? ToPlain(GraphValue? value) => value?.KindCase switch
    {
        null or GraphValue.KindOneofCase.None or GraphValue.KindOneofCase.Null => null,
        GraphValue.KindOneofCase.Boolean => value.Boolean,
        GraphValue.KindOneofCase.Int64 => value.Int64,
        GraphValue.KindOneofCase.Uint64 => value.Uint64,
        GraphValue.KindOneofCase.Float64 => value.Float64,
        GraphValue.KindOneofCase.String => value.String,
        GraphValue.KindOneofCase.Bytes => value.Bytes.ToByteArray(),
        GraphValue.KindOneofCase.Decimal => decimal.Parse(value.Decimal.Value, System.Globalization.CultureInfo.InvariantCulture),
        GraphValue.KindOneofCase.Vector => value.Vector.Values.ToArray(),
        GraphValue.KindOneofCase.List => value.List.Values.Select(ToPlain).ToList(),
        GraphValue.KindOneofCase.Map => value.Map.Entries.ToDictionary((kv) => kv.Key, (kv) => ToPlain(kv.Value)),
        _ => value,
    };
}
