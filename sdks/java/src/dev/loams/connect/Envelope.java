package dev.loams.connect;

import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;

/**
 * The Connect streaming envelope (the Connect protocol's "envelope framing").
 *
 * <p>Every message on a Connect server stream is a five-byte prefix followed by the payload:
 * one flag byte, then the payload's length as four bytes of big-endian unsigned. The same
 * framing carries the <b>end-of-stream</b> frame, whose payload is JSON rather than protobuf.
 *
 * <p>This is the one piece of the wire format the SDK implements itself rather than taking from
 * {@code connect-java}, because that artifact is not resolvable from Maven Central in this
 * environment (see {@code README.md}). It is small on purpose: the flag bits that matter are
 * {@link #FLAG_END_STREAM} and {@link #FLAG_COMPRESSED}, and the rest are rejected loudly
 * rather than skipped, so a server that starts using one produces a clear failure instead of a
 * stream that silently drops messages.
 */
public final class Envelope {

    /** The prefix's length in bytes: one flag plus a four-byte big-endian length. */
    public static final int PREFIX_LENGTH = 5;

    /** This frame is the end of the stream; its payload is a JSON stream result. */
    public static final int FLAG_END_STREAM = 0b0000_0010;

    /** This frame's payload is compressed. */
    public static final int FLAG_COMPRESSED = 0b0000_0001;

    private Envelope() {}

    /** Frame {@code payload} for sending, with {@link #FLAG_END_STREAM} clear. */
    public static byte[] frame(byte[] payload) {
        return frame(payload, 0);
    }

    /** Frame {@code payload} with {@code flags}, for sending. */
    public static byte[] frame(byte[] payload, int flags) {
        byte[] out = new byte[PREFIX_LENGTH + payload.length];
        out[0] = (byte) flags;
        int length = payload.length;
        out[1] = (byte) ((length >>> 24) & 0xff);
        out[2] = (byte) ((length >>> 16) & 0xff);
        out[3] = (byte) ((length >>> 8) & 0xff);
        out[4] = (byte) (length & 0xff);
        System.arraycopy(payload, 0, out, PREFIX_LENGTH, payload.length);
        return out;
    }

    /**
     * Read one frame's payload, or {@code null} at a clean end of stream.
     *
     * <p>A frame whose length runs past the end of the stream is an {@link EOFException} rather
     * than a silent truncation: a stream that stops mid-frame has lost a message, and a caller
     * that treated that as a clean end would believe it had seen everything.
     */
    public static Frame read(InputStream in) throws IOException {
        byte[] prefix = new byte[PREFIX_LENGTH];
        int read = readFully(in, prefix);
        if (read == 0) {
            return null;
        }
        if (read < PREFIX_LENGTH) {
            throw new EOFException(
                    "the stream ended after " + read + " bytes of a "
                            + PREFIX_LENGTH
                            + "-byte envelope prefix");
        }
        int flags = prefix[0] & 0xff;
        long length =
                ((long) (prefix[1] & 0xff) << 24)
                        | ((long) (prefix[2] & 0xff) << 16)
                        | ((long) (prefix[3] & 0xff) << 8)
                        | (prefix[4] & 0xff);
        if (length > Integer.MAX_VALUE) {
            throw new IOException("the envelope frame claims " + length + " bytes, which is not addressable");
        }
        byte[] payload = new byte[(int) length];
        if (readFully(in, payload) < payload.length) {
            throw new EOFException(
                    "the stream ended inside an envelope frame of " + payload.length + " bytes");
        }
        return new Frame(flags, payload);
    }

    /**
     * Read until the input is exhausted, treating a short read as an end.
     *
     * @return how many bytes were read, which is {@code length} or less only at a real end
     */
    private static int readFully(InputStream in, byte[] into) throws IOException {
        int total = 0;
        while (total < into.length) {
            int n = in.read(into, total, into.length - total);
            if (n < 0) {
                return total;
            }
            total += n;
        }
        return total;
    }

    /**
     * One framed message.
     *
     * @param flags the flag byte
     * @param payload the unframed payload: a serialized message, or a JSON stream result when
     *     {@link #isEndStream()}
     */
    public record Frame(int flags, byte[] payload) {

        /** Whether this is the end-of-stream frame rather than a message. */
        public boolean isEndStream() {
            return (flags & FLAG_END_STREAM) != 0;
        }

        /** Whether the payload is compressed, which this SDK does not negotiate. */
        public boolean isCompressed() {
            return (flags & FLAG_COMPRESSED) != 0;
        }

        /** The payload as UTF-8, for a frame whose payload is JSON. */
        public String payloadAsText() {
            return new String(payload, java.nio.charset.StandardCharsets.UTF_8);
        }
    }
}