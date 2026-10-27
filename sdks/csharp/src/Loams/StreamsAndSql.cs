// The `sql` and `streams` modules (SDKP §2.5, Part C): `loams.sql.v1` and
// `loams.stream.v1` on the main port (SDKP P0b and P0c).

using System.Runtime.CompilerServices;
using System.Text;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;
using Loams.Sql.V1;
using Loams.Stream.V1;
using CollectionConsistency = Loams.Collection.V1.Consistency;

namespace Loams;

/// <summary><c>loams.sql</c> — one read-only statement over a namespace's collections, with <c>$n</c> parameters.</summary>
public sealed class SqlModule
{
    private readonly CallInvoker _invoker;

    internal SqlModule(CallInvoker invoker) => _invoker = invoker;

    /// <summary>Runs one statement and answers its columns and rows.</summary>
    public Task<QueryResponse> QueryAsync(QueryRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<QueryResponse>(Facade.Binding("sql", "Query"), request, options, cancellationToken);

    /// <summary>
    /// Runs <paramref name="sql"/> with <paramref name="parameters"/> bound to <c>$1…$n</c>
    /// (a count mismatch is <c>invalid_argument</c>, reason <c>sql_param_count</c>).
    /// </summary>
    public Task<QueryResponse> QueryAsync(string sql, IEnumerable<object?>? parameters = null, uint? maxRows = null,
        CollectionConsistency? consistency = null, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var request = new QueryRequest { Sql = sql, Namespace = @namespace ?? string.Empty, Consistency = consistency };
        if (parameters is not null)
        {
            request.Params.AddRange(parameters.Select(Values.Of));
        }
        if (maxRows is { } rows)
        {
            request.MaxRows = rows;
        }
        return QueryAsync(request, options, cancellationToken);
    }

    /// <summary>
    /// Runs one statement and answers its result as one Arrow IPC stream
    /// (<see cref="QueryArrowResponse.ArrowIpc"/>); this SDK carries no Arrow reader, so
    /// the bytes are the caller's to read (Apache.Arrow's <c>ArrowStreamReader</c> does).
    /// </summary>
    public Task<QueryArrowResponse> QueryArrowAsync(QueryArrowRequest request, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        _invoker.UnaryAsync<QueryArrowResponse>(Facade.Binding("sql", "QueryArrow"), request, options, cancellationToken);
}

/// <summary>Reading SQL answers.</summary>
public static class SqlRows
{
    /// <summary>Each row as a dictionary of column name to plain value.</summary>
    public static IReadOnlyList<IReadOnlyDictionary<string, object?>> Records(this QueryResponse response)
    {
        ArgumentNullException.ThrowIfNull(response);
        var names = response.Columns.Select((c) => c.Name).ToList();
        return response.Rows
            .Select((row) => (IReadOnlyDictionary<string, object?>)names
                .Select((name, i) => (name, value: i < row.Values.Count ? Values.ToPlain(row.Values[i]) : null))
                .ToDictionary((p) => p.name, (p) => p.value, StringComparer.Ordinal))
            .ToList();
    }
}

/// <summary><c>loams.streams</c> — a namespace's logs: create, list, produce, fetch and subscribe.</summary>
public sealed class StreamsModule
{
    private readonly CallInvoker _invoker;

    internal StreamsModule(CallInvoker invoker) => _invoker = invoker;

    private Task<T> Call<T>(string name, IMessage request, CallOptions? options, CancellationToken ct)
        where T : IMessage<T>, new() =>
        _invoker.UnaryAsync<T>(Facade.Binding("streams", name), request, options, ct);

    /// <summary>Creates a stream. Idempotent by name and partition count.</summary>
    public Task<CreateStreamResponse> CreateAsync(string name, uint partitions = 1, string? @namespace = null,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        Call<CreateStreamResponse>("Create",
            new CreateStreamRequest { Name = name, Partitions = partitions, Namespace = @namespace ?? string.Empty },
            options, cancellationToken);

    /// <summary>One stream, with each partition's offsets.</summary>
    public Task<DescribeStreamResponse> GetAsync(string stream, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<DescribeStreamResponse>("Get",
            new DescribeStreamRequest { Stream = stream, Namespace = @namespace ?? string.Empty }, options, cancellationToken);

    /// <summary>One page of the namespace's streams.</summary>
    public Task<ListStreamsResponse> ListAsync(ListStreamsRequest? request = null, CallOptions? options = null,
        CancellationToken cancellationToken = default) =>
        Call<ListStreamsResponse>("List", request ?? new ListStreamsRequest(), options, cancellationToken);

    /// <summary>Every stream in the namespace, following the page tokens.</summary>
    public IAsyncEnumerable<StreamInfo> ListAllAsync(string? @namespace = null, int pageSize = 0,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        Paginator.ItemsAsync<ListStreamsRequest, ListStreamsResponse, StreamInfo>(
            Facade.Binding("streams", "List"),
            (request, token, ct) =>
            {
                var copy = request.Clone();
                copy.PageToken = token;
                return ListAsync(copy, options, ct);
            },
            new ListStreamsRequest { Namespace = @namespace ?? string.Empty, PageSize = pageSize },
            (page) => page.Streams, cancellationToken);

    /// <summary>
    /// Appends records to one partition. Each record is bytes, a string (UTF-8) or any
    /// other value (as JSON), or a <see cref="ProducerRecord"/> to give it a key and headers. Without <paramref name="producerId"/> a produce is a plain
    /// append and is never retried; with it and <paramref name="producerSeq"/> a repeat
    /// answers the first append with <c>duplicate</c> set, so it is retried.
    /// </summary>
    public Task<ProduceResponse> ProduceAsync(string stream, uint partition, IEnumerable<object> records,
        string? producerId = null, ulong? producerSeq = null, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var request = new ProduceRequest { Stream = stream, Partition = partition, Namespace = @namespace ?? string.Empty };
        request.Records.AddRange(records.Select(ToRecord));
        if (producerId is not null)
        {
            request.ProducerId = producerId;
        }
        if (producerSeq is { } seq)
        {
            request.ProducerSeq = seq;
        }
        var settings = options ?? CallOptions.None;
        if (producerId is not null && producerSeq is not null && settings.RetrySafe is null)
        {
            settings = settings with { RetrySafe = true };
        }
        return Call<ProduceResponse>("Produce", request, settings, cancellationToken);
    }

    /// <summary>One <c>loams.stream.v1.Record</c>: a <see cref="ProducerRecord"/> with its key and headers, or a bare value.</summary>
    internal static Record ToRecord(object record)
    {
        if (record is not ProducerRecord keyed)
        {
            return new Record { Value = ByteString.CopyFrom(Encode(record)) };
        }
        var output = new Record { Value = ByteString.CopyFrom(Encode(keyed.Value)) };
        if (keyed.Key is not null)
        {
            output.Key = ByteString.CopyFrom(Encode(keyed.Key));
        }
        foreach (var header in keyed.Headers ?? [])
        {
            output.Headers.Add(new Header { Key = header.Key, Value = ByteString.CopyFrom(Encode(header.Value)) });
        }
        return output;
    }

    /// <summary>
    /// A record's bytes: as given, a string's UTF-8, or a value's JSON with object keys sorted, so the
    /// same value is the same bytes however its dictionary was built.
    /// </summary>
    public static byte[] Encode(object record) => record switch
    {
        byte[] bytes => bytes,
        ByteString bytes => bytes.ToByteArray(),
        string text => Encoding.UTF8.GetBytes(text),
        _ => Encoding.UTF8.GetBytes(CompactJson.Format(Sorted(Values.Of(record)))),
    };

    private static Value Sorted(Value value) => value.KindCase switch
    {
        Value.KindOneofCase.StructValue => Value.ForStruct(SortedStruct(value.StructValue)),
        Value.KindOneofCase.ListValue => Value.ForList(value.ListValue.Values.Select(Sorted).ToArray()),
        _ => value,
    };

    private static Struct SortedStruct(Struct value)
    {
        var sorted = new Struct();
        foreach (var field in value.Fields.OrderBy((f) => f.Key, StringComparer.Ordinal))
        {
            sorted.Fields.Add(field.Key, Sorted(field.Value));
        }
        return sorted;
    }

    /// <summary>
    /// Reads one partition from <paramref name="offset"/>. <paramref name="maxWaitMs"/>
    /// holds an empty fetch until a record arrives (a long poll, at most 30 000).
    /// </summary>
    public Task<FetchResponse> FetchAsync(string stream, uint partition, ulong offset, uint? maxRecords = null,
        uint? maxBytes = null, uint? maxWaitMs = null, string? @namespace = null, CallOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        var request = new FetchRequest
        {
            Stream = stream,
            Partition = partition,
            Offset = offset,
            Namespace = @namespace ?? string.Empty,
        };
        if (maxRecords is { } n)
        {
            request.MaxRecords = n;
        }
        if (maxBytes is { } b)
        {
            request.MaxBytes = b;
        }
        if (maxWaitMs is { } w)
        {
            request.MaxWaitMs = w;
        }
        // A long poll may hold the call for its whole wait; the deadline covers it.
        var settings = options ?? CallOptions.None;
        if (maxWaitMs is { } wait && settings.Timeout is null)
        {
            settings = settings with { Timeout = _invoker.Timeout + TimeSpan.FromMilliseconds(wait) };
        }
        return Call<FetchResponse>("Fetch", request, settings, cancellationToken);
    }

    /// <summary>Appends CloudEvents (a JSON or protobuf batch).</summary>
    public Task<ProduceCloudEventsResponse> ProduceEventsAsync(ProduceCloudEventsRequest request,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        Call<ProduceCloudEventsResponse>("ProduceEvents", request, options, cancellationToken);

    /// <summary>Reads one partition's records as CloudEvents (a JSON batch).</summary>
    public Task<FetchCloudEventsResponse> FetchEventsAsync(FetchCloudEventsRequest request,
        CallOptions? options = null, CancellationToken cancellationToken = default) =>
        Call<FetchCloudEventsResponse>("FetchEvents", request, options, cancellationToken);

    /// <summary>
    /// A resumable subscription to one partition (R14): <c>await foreach</c> it, read
    /// <see cref="Subscription.Offset"/> to resume. Each empty fetch long-polls for
    /// <paramref name="maxWaitMs"/> and then backs off.
    /// </summary>
    public Subscription Subscribe(string stream, uint partition, ulong fromOffset = 0, uint maxWaitMs = 1000,
        string? @namespace = null, CallOptions? options = null)
    {
        return new Subscription(async (offset, ct) =>
        {
            var page = await FetchAsync(stream, partition, offset, maxWaitMs: maxWaitMs, @namespace: @namespace,
                options: options, cancellationToken: ct).ConfigureAwait(false);
            return page.Records
                .Select((r) => new StreamRecord(partition, r.Offset, r.Value?.ToByteArray() ?? [], (ulong)Math.Max(0, r.TimestampMs),
                    r.HasKey ? r.Key.ToByteArray() : null))
                .ToList();
        }, fromOffset);
    }

    /// <summary>The records of a subscription, as an async sequence (R14).</summary>
    public IAsyncEnumerable<StreamRecord> SubscribeAsync(string stream, uint partition, ulong fromOffset = 0,
        uint maxWaitMs = 1000, string? @namespace = null, CallOptions? options = null) =>
        Subscribe(stream, partition, fromOffset, maxWaitMs, @namespace, options);
}

/// <summary>
/// A record to produce with a key and headers (SDKP-25). The key (partitioning, compaction), the
/// value and each header value are bytes, a string (UTF-8) or any other value (as JSON), as
/// <see cref="StreamsModule.Encode"/> encodes them; a null key produces an unkeyed record.
/// </summary>
/// <param name="Key">The record's key, or null for none.</param>
/// <param name="Value">The record's value.</param>
/// <param name="Headers">Its headers, in order.</param>
public sealed record ProducerRecord(object? Key, object Value, IReadOnlyList<KeyValuePair<string, object>>? Headers = null);
