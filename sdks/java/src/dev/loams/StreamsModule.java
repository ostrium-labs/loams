package dev.loams;

import com.google.protobuf.ByteString;
import dev.loams.gen.loams.stream.v1.CreateStreamRequest;
import dev.loams.gen.loams.stream.v1.CreateStreamResponse;
import dev.loams.gen.loams.stream.v1.DescribeStreamRequest;
import dev.loams.gen.loams.stream.v1.DescribeStreamResponse;
import dev.loams.gen.loams.stream.v1.FetchCloudEventsRequest;
import dev.loams.gen.loams.stream.v1.FetchCloudEventsResponse;
import dev.loams.gen.loams.stream.v1.FetchRequest;
import dev.loams.gen.loams.stream.v1.FetchResponse;
import dev.loams.gen.loams.stream.v1.FetchedRecord;
import dev.loams.gen.loams.stream.v1.Header;
import dev.loams.gen.loams.stream.v1.ListStreamsRequest;
import dev.loams.gen.loams.stream.v1.ListStreamsResponse;
import dev.loams.gen.loams.stream.v1.ProduceCloudEventsRequest;
import dev.loams.gen.loams.stream.v1.ProduceCloudEventsResponse;
import dev.loams.gen.loams.stream.v1.ProduceRequest;
import dev.loams.gen.loams.stream.v1.ProduceResponse;
import dev.loams.gen.loams.stream.v1.Record;
import dev.loams.gen.loams.stream.v1.StreamInfo;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * {@code loams.streams} (SDKP §2.5, Part C): a namespace's logs over {@code loams.stream.v1} (SDKP
 * P0c): create, list, produce, fetch and subscribe.
 */
public final class StreamsModule {
    private final CallInvoker invoker;

    StreamsModule(CallInvoker invoker) {
        this.invoker = invoker;
    }

    private <Res extends com.google.protobuf.Message> Res call(
            com.google.protobuf.Parser<Res> parser, String name, com.google.protobuf.Message request, CallOptions options) {
        return invoker.unary(Modules.binding("streams", name), request, options, parser);
    }

    /** Creates a stream. Idempotent by name and partition count. */
    public CreateStreamResponse create(String name, int partitions, CallOptions... options) {
        return call(CreateStreamResponse.parser(), "Create",
                CreateStreamRequest.newBuilder().setName(name).setPartitions(partitions).build(), CallOptions.merge(options));
    }

    /** One stream, with each partition's offsets. */
    public DescribeStreamResponse get(String stream, CallOptions... options) {
        return call(DescribeStreamResponse.parser(), "Get",
                DescribeStreamRequest.newBuilder().setStream(stream).build(), CallOptions.merge(options));
    }

    /** One page of the namespace's streams. */
    public ListStreamsResponse list(ListStreamsRequest request, CallOptions... options) {
        return call(ListStreamsResponse.parser(), "List", request, CallOptions.merge(options));
    }

    /** Every stream in the namespace, following the page tokens. */
    public PageIterator<StreamInfo> listAll(int pageSize, CallOptions... options) {
        return PageIterator.of(Modules.binding("streams", "List"),
                token -> list(ListStreamsRequest.newBuilder().setPageSize(pageSize).setPageToken(token).build(), options));
    }

    /** A record's bytes: as given, a string's UTF-8, or a value's JSON. */
    public static byte[] encode(Object record) {
        if (record instanceof byte[] bytes) {
            return bytes;
        }
        if (record instanceof ByteString bytes) {
            return bytes.toByteArray();
        }
        if (record instanceof String text) {
            return text.getBytes(StandardCharsets.UTF_8);
        }
        return Values.json(Values.of(record)).getBytes(StandardCharsets.UTF_8);
    }

    /** One {@code loams.stream.v1.Record}: a {@link ProducerRecord} with its key and headers, or a bare value. */
    static Record record(Object record) {
        if (!(record instanceof ProducerRecord keyed)) {
            return Record.newBuilder().setValue(ByteString.copyFrom(encode(record))).build();
        }
        Record.Builder out = Record.newBuilder().setValue(ByteString.copyFrom(encode(keyed.value())));
        if (keyed.key() != null) {
            out.setKey(ByteString.copyFrom(encode(keyed.key())));
        }
        for (Map.Entry<String, ?> header : keyed.headers().entrySet()) {
            out.addHeaders(Header.newBuilder().setKey(header.getKey())
                    .setValue(ByteString.copyFrom(encode(header.getValue()))));
        }
        return out.build();
    }

    /**
     * Appends records to one partition. Each record is bytes, a string (UTF-8) or a value (as JSON),
     * or a {@link ProducerRecord} to give it a key and headers.
     * With {@code producerId} and {@code producerSeq} (both or neither) a repeat answers the first
     * append with {@code duplicate} set, so the SDK retries it; without them a produce is a plain
     * append and is never retried.
     */
    public ProduceResponse produce(String stream, int partition, List<?> records, String producerId, Long producerSeq,
            CallOptions... options) {
        ProduceRequest.Builder request = ProduceRequest.newBuilder().setStream(stream).setPartition(partition);
        for (Object record : records) {
            request.addRecords(record(record));
        }
        CallOptions merged = CallOptions.merge(options);
        if (producerId != null && producerSeq != null) {
            request.setProducerId(producerId).setProducerSeq(producerSeq);
            if (merged.retrySafe() == null) {
                merged = CallOptions.builder().from(merged).retrySafe(true).build();
            }
        }
        return call(ProduceResponse.parser(), "Produce", request.build(), merged);
    }

    /** Appends records as a plain, unkeyed append. */
    public ProduceResponse produce(String stream, int partition, List<?> records, CallOptions... options) {
        return produce(stream, partition, records, null, null, options);
    }

    /**
     * Reads one partition from {@code offset}; {@code maxWaitMs} (null for none) holds an empty
     * fetch until a record arrives (a long poll, at most 30 000).
     */
    public FetchResponse fetch(String stream, int partition, long offset, Integer maxRecords, Integer maxWaitMs,
            CallOptions... options) {
        FetchRequest.Builder request = FetchRequest.newBuilder().setStream(stream).setPartition(partition).setOffset(offset);
        if (maxRecords != null) {
            request.setMaxRecords(maxRecords);
        }
        CallOptions merged = CallOptions.merge(options);
        if (maxWaitMs != null) {
            request.setMaxWaitMs(maxWaitMs);
            if (merged.timeout() == null) {
                merged = CallOptions.builder().from(merged)
                        .timeout(invoker.timeout().plusMillis(maxWaitMs)).build();
            }
        }
        return call(FetchResponse.parser(), "Fetch", request.build(), merged);
    }

    /** Appends CloudEvents (a JSON or protobuf batch). */
    public ProduceCloudEventsResponse produceEvents(ProduceCloudEventsRequest request, CallOptions... options) {
        return call(ProduceCloudEventsResponse.parser(), "ProduceEvents", request, CallOptions.merge(options));
    }

    /** Reads one partition's records as CloudEvents (a JSON batch). */
    public FetchCloudEventsResponse fetchEvents(FetchCloudEventsRequest request, CallOptions... options) {
        return call(FetchCloudEventsResponse.parser(), "FetchEvents", request, CallOptions.merge(options));
    }

    /**
     * A resumable subscription to one partition (R14): iterate it, read {@link Subscription#offset()}
     * to resume, close it to stop. Each empty fetch long-polls for {@code maxWaitMs}, then backs off.
     */
    public Subscription subscribe(String stream, int partition, long fromOffset, int maxWaitMs, CallOptions... options) {
        return new Subscription(offset -> {
            List<Subscription.Record> out = new ArrayList<>();
            for (FetchedRecord r : fetch(stream, partition, offset, null, maxWaitMs, options).getRecordsList()) {
                out.add(new Subscription.Record(partition, r.getOffset(), r.getValue().toByteArray(), r.getTimestampMs(),
                        r.hasKey() ? r.getKey().toByteArray() : null));
            }
            return out;
        }, fromOffset);
    }
}
