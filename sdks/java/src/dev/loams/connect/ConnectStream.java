package dev.loams.connect;

import dev.loams.Protocol;
import java.io.IOException;
import java.io.InputStream;

/**
 * One open server stream, read as raw serialized messages.
 *
 * <p>It exists to own the two things the SDK's {@link dev.loams.Stream} must not have to get
 * right itself:
 *
 * <ul>
 *   <li><b>The envelope.</b> Every message is a five-byte prefix and a payload, and the end of
 *       the stream is its own frame.
 *   <li><b>The failure.</b> On the Connect protocol a refusal arrives inside the end-of-stream
 *       frame on a 200; on gRPC and gRPC-Web it arrives in the trailers. Reading either and
 *       reporting it as a failure is what keeps a stream's refusal from looking like a stream
 *       that simply ended, which would leave the caller unable to tell "nothing changed" from
 *       "this feature is not in this build" (R5, R8).
 * </ul>
 *
 * <p>The protobuf decoding is deliberately <em>not</em> here: this type yields bytes so the wire
 * layer needs no knowledge of any message type, and {@code dev.loams.Stream} does the decoding.
 */
public final class ConnectStream implements AutoCloseable {

    /** gRPC-Web's trailer-frame flag. The Connect end-of-stream flag shares bit 2, not bit 7. */
    private static final int FLAG_TRAILER = 0x80;

    private final String rpc;
    private final InputStream body;
    private final String trailerBlock;
    private final Protocol protocol;

    private boolean ended;

    ConnectStream(String rpc, InputStream body, String trailerBlock, Protocol protocol) {
        this.rpc = rpc;
        this.body = body;
        this.trailerBlock = trailerBlock;
        this.protocol = protocol;
    }

    /**
     * The next message's bytes.
     *
     * @return the next message, or {@code null} when the stream ended cleanly
     * @throws ConnectFailure when the stream ended in a failure, on any of the three protocols
     * @throws IOException when the connection broke
     */
    public byte[] next() throws IOException {
        if (ended) {
            return null;
        }
        while (true) {
            Envelope.Frame frame = Envelope.read(body);
            if (frame == null) {
                // The body ran out with no end-of-stream frame. On the Connect protocol that is
                // a truncated stream rather than a clean end, and treating it as clean would
                // report "no more messages" for a stream that lost one.
                ended = true;
                if (protocol == Protocol.CONNECT) {
                    throw new ConnectFailure(
                            rpc,
                            null,
                            new IOException("the stream ended without an end-of-stream frame"));
                }
                return checkGrpcEnd(null);
            }
            if ((frame.flags() & FLAG_TRAILER) != 0) {
                ended = true;
                return checkGrpcEnd(frame.payloadAsText());
            }
            if (frame.isCompressed()) {
                throw new ConnectFailure(
                        rpc,
                        null,
                        new IOException(
                                "the server sent a compressed stream frame, which this SDK does not negotiate"));
            }
            if (frame.isEndStream()) {
                ended = true;
                String payload = frame.payloadAsText();
                if (protocol == Protocol.CONNECT) {
                    ConnectError error = ConnectError.fromEndStream(payload);
                    if (error == null) {
                        return null;
                    }
                    throw new ConnectFailure(rpc, error, null);
                }
                return checkGrpcEnd(payload);
            }
            return frame.payload();
        }
    }

    /** The gRPC end: {@code null} on success, the failure otherwise. */
    private byte[] checkGrpcEnd(String trailerText) throws IOException {
        String block = trailerText != null ? trailerText : trailerBlock;
        if (block == null) {
            // No trailers at all. gRPC requires them, so the stream cannot be declared successful.
            throw new ConnectFailure(
                    rpc, null, new IOException("the stream ended without gRPC trailers"));
        }
        String statusText = headerValue(block, "grpc-status");
        if (statusText == null) {
            throw new ConnectFailure(
                    rpc, null, new IOException("the gRPC trailers carry no grpc-status"));
        }
        int status;
        try {
            status = Integer.parseInt(statusText.trim());
        } catch (NumberFormatException e) {
            throw new ConnectFailure(
                    rpc, null, new IOException("the gRPC trailers carry grpc-status " + statusText));
        }
        if (status == 0) {
            return null;
        }
        String message = headerValue(block, "grpc-message");
        GrpcStatus.Status carried = GrpcStatus.parse(headerValue(block, "grpc-status-details-bin"));
        ConnectError error;
        if (carried != null && !carried.details().isEmpty()) {
            error = new ConnectError(
                    dev.loams.Code.fromNumber(carried.code()).wire(),
                    message != null && !message.isEmpty() ? message : carried.message(),
                    carried.details(),
                    java.util.Map.of());
        } else {
            error = new ConnectError(
                    dev.loams.Code.fromNumber(status).wire(),
                    message == null ? "" : message,
                    java.util.List.of(),
                    java.util.Map.of());
        }
        throw new ConnectFailure(rpc, error, null);
    }

    private static String headerValue(String block, String name) {
        String wanted = name.toLowerCase(java.util.Locale.ROOT);
        for (String line : block.split("\r\n|\n")) {
            int colon = line.indexOf(':');
            if (colon > 0 && line.substring(0, colon).trim().toLowerCase(java.util.Locale.ROOT).equals(wanted)) {
                return line.substring(colon + 1).trim();
            }
        }
        return null;
    }

    /** Whether {@link #next()} has already reported the end. */
    public boolean isEnded() {
        return ended;
    }

    /** Close the stream, releasing the connection. Safe to call more than once. */
    @Override
    public void close() {
        try {
            body.close();
        } catch (IOException e) {
            // Closing is best effort: the caller is already finished with the stream, and a
            // failure to release a pooled connection is not something they can act on.
        }
    }
}