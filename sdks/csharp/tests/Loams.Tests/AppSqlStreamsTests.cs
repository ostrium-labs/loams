// SDKP Part C (L5): the `sql` and `streams` modules over `loams.sql.v1` and
// `loams.stream.v1`, against stubs. The recorded `app_sql` and `app_streams`
// scenarios replace these once P0d records them.

using System.Net;
using System.Text;
using Loams.Sql.V1;
using Loams.Stream.V1;
using Loams.Tests.Support;
using Xunit;
using static Loams.Tests.AppRuntimeTests;

namespace Loams.Tests;

public sealed class AppSqlStreamsTests
{
    [Fact]
    public async Task csharp_sql_params_bind_and_rows_read_as_records()
    {
        var handler = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK,
            """{"columns":[{"name":"tags","type":"Utf8"},{"name":"c","type":"Int64"}],"rows":[["a",2],["b",1]]}"""));
        using var client = ClientOver(handler);

        var answer = await client.Sql.QueryAsync(
            "SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", [1]);

        var sent = Body(handler.Requests[0], QueryRequest.Parser);
        Assert.Equal("app", sent.Namespace);
        Assert.Equal(1.0, Assert.Single(sent.Params).NumberValue);
        Assert.Equal("/loams.sql.v1.SqlService/Query", handler.Requests[0].Path);
        var records = answer.Records();
        Assert.Equal("a", records[0]["tags"]);
        Assert.Equal(2.0, records[0]["c"]);
        Assert.Equal(RetryClass.Safe, Facade.Binding("sql", "Query").Retry);
    }

    [Fact]
    public async Task csharp_streams_produce_is_retried_only_with_a_producer_key()
    {
        var unkeyed = new StubHandler(
            StubHandler.ConnectRefusal(HttpStatusCode.ServiceUnavailable, "unavailable", "restarting", "unavailable"),
            StubHandler.ConnectJson(HttpStatusCode.OK, """{"baseOffset":"0","lastOffset":"0"}"""));
        using (var client = ClientOver(unkeyed))
        {
            await Assert.ThrowsAnyAsync<LoamsError>(() => client.Streams.ProduceAsync("events", 0, ["a"]));
        }
        Assert.Equal(1, unkeyed.CallCount);

        var keyed = new StubHandler(
            StubHandler.ConnectRefusal(HttpStatusCode.ServiceUnavailable, "unavailable", "restarting", "unavailable"),
            StubHandler.ConnectJson(HttpStatusCode.OK, """{"baseOffset":"0","lastOffset":"2"}"""));
        using (var client = ClientOver(keyed))
        {
            var produced = await client.Streams.ProduceAsync("events", 0,
                ["text", new byte[] { 1, 2 }, new Dictionary<string, object?> { ["n"] = 1 }], producerId: "app-1", producerSeq: 7);
            Assert.Equal(2UL, produced.LastOffset);
        }
        Assert.Equal(2, keyed.CallCount);
        var sent = Body(keyed.Requests[0], ProduceRequest.Parser);
        Assert.Equal(["text", "\u0001\u0002", "{\"n\":1}"], sent.Records.Select((r) => Encoding.UTF8.GetString(r.Value.ToByteArray())));
        Assert.Equal("app-1", sent.ProducerId);
        Assert.Equal(7UL, sent.ProducerSeq);
    }

    /// <summary>Keyed records (SDKP-25) carry the recorded app_streams produce body's keys and values.</summary>
    [Fact]
    public async Task csharp_streams_produce_keyed_records()
    {
        var handler = new StubHandler(StubHandler.ConnectJson(HttpStatusCode.OK, """{"baseOffset":"0","lastOffset":"1"}"""));
        using (var client = ClientOver(handler))
        {
            await client.Streams.ProduceAsync("note-events", 0,
            [
                new ProducerRecord("1", new Dictionary<string, object?> { ["op"] = "created", ["note"] = 1 }),
                new ProducerRecord("5", new Dictionary<string, object?> { ["op"] = "deleted", ["note"] = 5 },
                    [new KeyValuePair<string, object>("source", "notes")]),
                "plain",
            ], producerId: "app-producer-1", producerSeq: 1);
        }
        var sent = Body(handler.Requests[0], ProduceRequest.Parser);
        Assert.Equal("MQ==", Convert.ToBase64String(sent.Records[0].Key.ToByteArray()));
        Assert.Equal("eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ==", Convert.ToBase64String(sent.Records[0].Value.ToByteArray()));
        Assert.Equal("eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ==", Convert.ToBase64String(sent.Records[1].Value.ToByteArray()));
        Assert.Equal("source", sent.Records[1].Headers[0].Key);
        Assert.Equal("notes", Encoding.UTF8.GetString(sent.Records[1].Headers[0].Value.ToByteArray()));
        Assert.False(sent.Records[2].HasKey);
    }

    [Fact]
    public async Task csharp_streams_subscribe_reads_through_fetch()
    {
        var handler = new StubHandler(
            StubHandler.ConnectJson(HttpStatusCode.OK, """{"records":[{"offset":"1","value":"YQ=="},{"offset":"2","value":"Yg=="}],"nextOffset":"3"}"""),
            StubHandler.ConnectJson(HttpStatusCode.OK, """{"records":[{"offset":"3","value":"Yw=="}],"nextOffset":"4"}"""));
        using var client = ClientOver(handler);
        var subscription = client.Streams.Subscribe("events", 0, fromOffset: 1, maxWaitMs: 500);
        var seen = new List<string>();
        await foreach (var record in subscription)
        {
            seen.Add(record.Text);
            if (seen.Count == 3)
            {
                break;
            }
        }
        Assert.Equal(["a", "b", "c"], seen);
        Assert.Equal(4UL, subscription.Offset);
        var first = Body(handler.Requests[0], FetchRequest.Parser);
        Assert.Equal(1UL, first.Offset);
        Assert.Equal(500U, first.MaxWaitMs);
        Assert.Equal(3UL, Body(handler.Requests[1], FetchRequest.Parser).Offset);
    }
}
