package dev.loams;

/** The wire protocol a client speaks. All three are served on one port (design §44 §4, D600). */
public enum Protocol {
    /**
     * The Connect protocol: the default.
     *
     * <p>It is the only one that works identically over HTTP/1.1 and HTTP/2, and its unary form
     * is an HTTP {@code POST} with a body a person can read, which is what {@code curl} sends
     * (design §44 §4). Its streaming form frames every message, and carries the failure
     * <em>inside</em> the end-of-stream frame rather than as an HTTP status — which is why the
     * SDK reads the envelope and not just the status code.
     */
    CONNECT("connect"),
    /**
     * gRPC over HTTP/2.
     *
     * <p>It needs a listener that speaks HTTP/2 with a real TLS ALPN, or h2c on a plaintext
     * loopback stack.
     */
    GRPC("grpc"),
    /**
     * gRPC-Web.
     *
     * <p>A JVM client rarely wants it — a browser cannot do gRPC and a JVM program can — but it
     * is served on the same port, and a caller behind a proxy that only speaks gRPC-Web needs
     * it. See R10: the browser is a first-class target, and gRPC-Web is the framing it can
     * speak.
     */
    GRPC_WEB("grpc-web");

    private final String wire;

    Protocol(String wire) {
        this.wire = wire;
    }

    /** The lowercase name, as {@code Protocol} spells it in the Go SDK. */
    public String wire() {
        return wire;
    }

    @Override
    public String toString() {
        return wire;
    }
}