package dev.loams.connect;

import java.io.IOException;

/**
 * A failure that came back over the wire, before it is mapped to the SDK's exception
 * hierarchy.
 *
 * <p>It extends {@link IOException} so it travels through the JDK client's own checked-exception
 * path without the transport having to know that {@link dev.loams.LoamsException} exists. The
 * mapper in {@link dev.loams.Errors} turns it into the typed hierarchy; nothing above the
 * transport should catch this type.
 */
public class ConnectFailure extends IOException {

    private static final long serialVersionUID = 1L;

    private final String rpc;
    private final ConnectError error;

    /**
     * @param rpc the failing RPC, as {@code package.Service/Method}
     * @param error what the server said, or {@code null} when the body was not a Connect error
     *     at all — a proxy's HTML page, for instance. {@code null} is the honest reading, and
     *     {@link dev.loams.Errors} reports it as a below-the-API failure.
     * @param cause the transport failure underneath, when there was one
     */
    public ConnectFailure(String rpc, ConnectError error, Throwable cause) {
        super(describe(rpc, error), cause);
        this.rpc = rpc;
        this.error = error;
    }

    /** The failing RPC, as {@code package.Service/Method}. */
    public final String rpc() {
        return rpc;
    }

    /** What the server said, or {@code null} when the body was not a Connect error. */
    public final ConnectError error() {
        return error;
    }

    private static String describe(String rpc, ConnectError error) {
        StringBuilder out = new StringBuilder();
        if (rpc != null && !rpc.isEmpty()) {
            out.append(rpc).append(": ");
        }
        if (error == null) {
            out.append("the response body was not a Connect error");
        } else {
            out.append(error.code() == null ? "an unrecognised code" : error.code());
            if (error.message() != null && !error.message().isEmpty()) {
                out.append(": ").append(error.message());
            }
        }
        return out.toString();
    }
}