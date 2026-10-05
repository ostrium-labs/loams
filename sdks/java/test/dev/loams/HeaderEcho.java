package dev.loams;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.util.List;

/**
 * An endpoint that reports back what it saw, for the R1 test that asserts the bearer travels in a
 * header and <b>not</b> in the URL.
 *
 * <p>A stub rather than the fixture corpus because the corpus's {@code GetInstance} recording does
 * not echo headers, and "the token is not in the query string" is only checkable by looking at what
 * actually went out on the wire. It answers {@code loams.instance.v1.InstanceService/GetInstance}
 * with the recorded body so the client's own parse succeeds and the test can assert on the
 * request rather than on an error.
 */
final class HeaderEcho implements AutoCloseable {

    private final HttpServer server;

    /**
     * What the endpoint saw. Read after the call, never concurrently.
     *
     * <p>It is reached through {@link #self} rather than through {@code this}, because the
     * handler is a lambda and the server has to exist before the instance that wraps it.
     */
    private volatile Seen seen;

    private HeaderEcho(HttpServer server) {
        this.server = server;
    }

    /** Start on an ephemeral loopback port. */
    static HeaderEcho start() {
        HttpServer server;
        try {
            server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        } catch (IOException e) {
            throw new AssertionError("could not start the header echo: " + e, e);
        }
        server.setExecutor(null);
        HeaderEcho echo = new HeaderEcho(server);
        // The handler writes to the instance's field, and the server has to exist before the
        // instance that wraps it, so the two are joined through a holder the lambda closes over.
        final HeaderEcho[] holder = {echo};
        server.createContext(
                "/",
                exchange -> {
                    holder[0].seen = read(exchange);
                    answer(exchange);
                });
        server.start();
        return echo;
    }

    String endpoint() {
        return "http://127.0.0.1:" + server.getAddress().getPort();
    }

    /** What the last request carried. */
    Seen seen() {
        return seen;
    }

    private static Seen read(HttpExchange exchange) throws IOException {
        // The body has to be drained before answering, or the exchange is closed rather than
        // replied to.
        try (var in = exchange.getRequestBody()) {
            while (in.read() >= 0) {
                continue;
            }
        }
        List<String> authorization = exchange.getRequestHeaders().get("Authorization");
        String first = authorization == null || authorization.isEmpty() ? null : authorization.get(0);
        String duplicate = null;
        if (authorization != null && authorization.size() > 1) {
            duplicate = String.join(", ", authorization.subList(1, authorization.size()));
        }
        String contentType = exchange.getRequestHeaders().getFirst("content-type");
        return new Seen(
                exchange.getRequestMethod(),
                exchange.getRequestURI().toString(),
                first,
                duplicate,
                contentType);
    }

    /** The recorded {@code GetInstance} response, so the client's parse succeeds. */
    private static void answer(HttpExchange exchange) throws IOException {
        // Replayed from the corpus rather than hand-built, so the bytes the client parses are the
        // bytes every other SDK's suite parses.
        byte[] body = Fixtures.readRecordedBody("instance_get_instance_proto");
        exchange.getResponseHeaders().set("content-type", "application/proto");
        exchange.sendResponseHeaders(200, body.length);
        try (OutputStream out = exchange.getResponseBody()) {
            out.write(body);
        }
    }

    @Override
    public void close() {
        server.stop(0);
    }

    /**
     * What one request carried.
     *
     * @param method the HTTP method
     * @param uri the full request target, which is what a query-string leak would show up in
     * @param authorization the first {@code Authorization} header, or {@code null}
     * @param duplicateAuthorization any further {@code Authorization} headers, which the runtime
     *     must not produce — a stale bearer stacking up beside a refreshed one is the bug the
     *     replace-not-add rule exists to prevent
     * @param contentType the request's content type
     */
    record Seen(
            String method,
            String uri,
            String authorization,
            String duplicateAuthorization,
            String contentType) {}
}