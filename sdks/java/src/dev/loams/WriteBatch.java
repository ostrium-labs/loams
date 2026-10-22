package dev.loams;

import dev.loams.gen.loams.collection.v1.WriteDocumentsResponse;
import dev.loams.gen.loams.collection.v1.WriteOp;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.NoSuchElementException;

/**
 * {@code documents.write_batch} (runtime contract R13, SDKP §2.3, design §44 §7.5).
 *
 * <p>Any number of write ops, cut into chunks of at most {@code chunkRows} ops and about {@code
 * chunkBytes} bytes, sent one chunk at a time. Each chunk gets its own idempotency key, minted once
 * and reused on that chunk's retries, so a retried chunk is the same write. The tokens merge into
 * one. The first chunk that fails stops the batch with a {@link BatchException} that says how many
 * chunks were applied, so a caller can resume from there.
 */
public final class WriteBatch {

    private WriteBatch() {}

    /** At most this many ops per chunk by default. */
    public static final int DEFAULT_CHUNK_ROWS = 10_000;

    /** About this many bytes per chunk by default (4 MiB). */
    public static final int DEFAULT_CHUNK_BYTES = 4 * 1024 * 1024;

    /** The client-side reason a stopped batch carries (docs/api/reasons.md). */
    public static final String BATCH_PARTIALLY_APPLIED = "batch_partially_applied";

    /** A finished batch: the merged token and how many chunks and ops it wrote. */
    public record Result(String token, int chunks, int ops) {}

    /** Sends one chunk with its key. */
    @FunctionalInterface
    public interface ChunkSender {
        WriteDocumentsResponse send(List<WriteOp> chunk, String idempotencyKey);
    }

    /**
     * A batch that stopped at a failed chunk (reason {@code batch_partially_applied}, client-side).
     * The failure is the cause.
     */
    public static final class BatchException extends LoamsException {
        private static final long serialVersionUID = 1L;
        private final int appliedChunks;
        private final int appliedOps;
        private final String token;

        BatchException(int appliedChunks, int appliedOps, String token, LoamsException cause) {
            super(cause.code(), cause.reason(), BATCH_PARTIALLY_APPLIED,
                    Map.of("applied_chunks", Integer.toString(appliedChunks), "applied_ops", Integer.toString(appliedOps)),
                    cause.hint(), cause.rpc(), cause, false);
            this.appliedChunks = appliedChunks;
            this.appliedOps = appliedOps;
            this.token = token;
        }

        /** The chunks applied before the failure. */
        public int appliedChunks() {
            return appliedChunks;
        }

        /** The ops applied before the failure; resume from this index. */
        public int appliedOps() {
            return appliedOps;
        }

        /** The merged token of the applied chunks. */
        public String token() {
            return token;
        }
    }

    /** Cuts {@code ops} into chunks and sends them in order, one at a time. */
    public static Result run(Iterable<WriteOp> ops, int chunkRows, int chunkBytes, ChunkSender send) {
        if (chunkRows < 1 || chunkBytes < 1) {
            throw new IllegalArgumentException("loams: write_batch chunks need at least one row and one byte");
        }
        ConsistencyToken merged = null;
        String foreign = null;
        int chunks = 0;
        int applied = 0;
        for (List<WriteOp> chunk : chunks(ops, chunkRows, chunkBytes)) {
            WriteDocumentsResponse answer;
            try {
                answer = send.send(chunk, Idempotency.uuidV7());
            } catch (LoamsException error) {
                throw new BatchException(chunks, applied, print(merged, foreign), error);
            }
            String token = answer.getToken();
            if (!token.isEmpty()) {
                var parsed = ConsistencyToken.tryParse(token);
                if (parsed.isPresent()) {
                    merged = merged == null ? parsed.get() : merged.merge(parsed.get());
                } else {
                    foreign = token;
                }
            }
            chunks++;
            applied += chunk.size();
        }
        return new Result(print(merged, foreign), chunks, applied);
    }

    private static String print(ConsistencyToken merged, String foreign) {
        return merged != null ? merged.toString() : foreign != null ? foreign : "";
    }

    /** The chunks: at most {@code rows} ops, and a new chunk once the next op would pass {@code bytes}. */
    public static Iterable<List<WriteOp>> chunks(Iterable<WriteOp> ops, int rows, int bytes) {
        return () -> new Iterator<>() {
            private final Iterator<WriteOp> source = ops.iterator();
            private WriteOp carry;

            @Override
            public boolean hasNext() {
                return carry != null || source.hasNext();
            }

            @Override
            public List<WriteOp> next() {
                if (!hasNext()) {
                    throw new NoSuchElementException();
                }
                List<WriteOp> chunk = new ArrayList<>();
                int size = 0;
                while (carry != null || source.hasNext()) {
                    WriteOp op = carry != null ? carry : source.next();
                    carry = null;
                    int opSize = op.getSerializedSize();
                    if (!chunk.isEmpty() && (chunk.size() >= rows || size + opSize > bytes)) {
                        carry = op;
                        break;
                    }
                    chunk.add(op);
                    size += opSize;
                }
                return chunk;
            }
        };
    }
}
