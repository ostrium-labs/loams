package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.fail;

import com.google.protobuf.ByteString;
import com.google.protobuf.ListValue;
import com.google.protobuf.Value;
import dev.loams.gen.loams.sql.v1.Column;
import dev.loams.gen.loams.sql.v1.QueryRequest;
import dev.loams.gen.loams.sql.v1.QueryResponse;
import dev.loams.gen.loams.stream.v1.FetchRequest;
import dev.loams.gen.loams.stream.v1.FetchResponse;
import dev.loams.gen.loams.stream.v1.FetchedRecord;
import dev.loams.gen.loams.stream.v1.ProduceRequest;
import dev.loams.gen.loams.stream.v1.ProduceResponse;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import org.junit.Test;

/** SDKP Part C (L8): the {@code sql} and {@code streams} modules, against a stub Connect server. */
public class AppSqlStreamsTest {

    @Test
    public void java_sql_params_bind_and_rows_read_as_records() throws Exception {
        QueryResponse rows = QueryResponse.newBuilder()
                .addColumns(Column.newBuilder().setName("tags").setType("Utf8"))
                .addColumns(Column.newBuilder().setName("c").setType("Int64"))
                .addRows(ListValue.newBuilder().addValues(Value.newBuilder().setStringValue("a")).addValues(Value.newBuilder().setNumberValue(2)))
                .build();
        try (AppStub stub = new AppStub(AppStub.ok(rows))) {
            Client client = stub.client("app", false);
            QueryResponse answer = client.sql().query("SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags", List.of(1));
            QueryRequest sent = QueryRequest.parseFrom(stub.requests.get(0).body());
            assertEquals("app", sent.getNamespace());
            assertEquals(1.0, sent.getParams(0).getNumberValue(), 0);
            assertEquals("/loams.sql.v1.SqlService/Query", stub.requests.get(0).path());
            assertEquals(List.of(Map.of("tags", "a", "c", 2.0)), SqlModule.records(answer));
        }
    }

    /** Keyed records (SDKP-25) carry the recorded app_streams produce body's keys and values. */
    @Test
    public void java_streams_produce_keyed_records() throws Exception {
        try (AppStub stub = new AppStub(AppStub.ok(ProduceResponse.newBuilder().setLastOffset(1).build()))) {
            stub.client("app", false).streams().produce("note-events", 0, List.of(
                    ProducerRecord.of("1", Map.of("op", "created", "note", 1)),
                    new ProducerRecord("5", Map.of("op", "deleted", "note", 5), Map.of("source", "notes")),
                    "plain"), "app-producer-1", 1L);
            ProduceRequest sent = ProduceRequest.parseFrom(stub.requests.get(0).body());
            assertEquals(3, sent.getRecordsCount());
            assertEquals("MQ==", java.util.Base64.getEncoder().encodeToString(sent.getRecords(0).getKey().toByteArray()));
            assertEquals("eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ==",
                    java.util.Base64.getEncoder().encodeToString(sent.getRecords(0).getValue().toByteArray()));
            assertEquals("eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ==",
                    java.util.Base64.getEncoder().encodeToString(sent.getRecords(1).getValue().toByteArray()));
            assertEquals("source", sent.getRecords(1).getHeaders(0).getKey());
            assertEquals("notes", sent.getRecords(1).getHeaders(0).getValue().toString(StandardCharsets.UTF_8));
            assertEquals(false, sent.getRecords(2).hasKey());
        }
    }

    @Test
    public void java_streams_produce_is_retried_only_with_a_producer_key() throws Exception {
        ProduceResponse ok = ProduceResponse.newBuilder().setLastOffset(2).build();
        try (AppStub stub = new AppStub(AppStub.refusal(503, "unavailable", "restarting"), AppStub.ok(ok))) {
            try {
                stub.client("app", false).streams().produce("events", 0, List.of("a"));
                fail("an unkeyed produce was retried or succeeded");
            } catch (LoamsException expected) {
                assertEquals(1, stub.requests.size());
            }
        }
        try (AppStub stub = new AppStub(AppStub.refusal(503, "unavailable", "restarting"), AppStub.ok(ok))) {
            ProduceResponse answer = stub.client("app", false).streams()
                    .produce("events", 0, List.of("text", new byte[] {1, 2}, Map.of("n", 1)), "app-1", 7L);
            assertEquals(2, answer.getLastOffset());
            assertEquals(2, stub.requests.size());
            ProduceRequest sent = ProduceRequest.parseFrom(stub.requests.get(0).body());
            List<String> values = new ArrayList<>();
            sent.getRecordsList().forEach(r -> values.add(r.getValue().toString(StandardCharsets.UTF_8)));
            assertEquals(List.of("text", "\u0001\u0002", "{\"n\":1}"), values);
            assertEquals("app-1", sent.getProducerId());
            assertEquals(7, sent.getProducerSeq());
        }
    }

    @Test
    public void java_streams_subscribe_reads_through_fetch() throws Exception {
        FetchResponse first = FetchResponse.newBuilder()
                .addRecords(FetchedRecord.newBuilder().setOffset(1).setValue(ByteString.copyFromUtf8("a")))
                .addRecords(FetchedRecord.newBuilder().setOffset(2).setValue(ByteString.copyFromUtf8("b")))
                .setNextOffset(3).build();
        FetchResponse second = FetchResponse.newBuilder()
                .addRecords(FetchedRecord.newBuilder().setOffset(3).setValue(ByteString.copyFromUtf8("c")))
                .setNextOffset(4).build();
        try (AppStub stub = new AppStub(AppStub.ok(first), AppStub.ok(second))) {
            Subscription subscription = stub.client("app", false).streams().subscribe("events", 0, 1, 500);
            List<String> seen = new ArrayList<>();
            while (seen.size() < 3 && subscription.hasNext()) {
                seen.add(subscription.next().text());
            }
            subscription.close();
            assertEquals(List.of("a", "b", "c"), seen);
            assertEquals(4, subscription.offset());
            FetchRequest sent = FetchRequest.parseFrom(stub.requests.get(0).body());
            assertEquals(1, sent.getOffset());
            assertEquals(500, sent.getMaxWaitMs());
            assertEquals(3, FetchRequest.parseFrom(stub.requests.get(1).body()).getOffset());
        }
    }
}
