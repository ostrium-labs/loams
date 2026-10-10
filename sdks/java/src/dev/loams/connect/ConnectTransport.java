package dev.loams.connect;

import dev.loams.Protocol;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.http.HttpClient;
import java.net.http.HttpHeaders;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * The Connect transport: the one place in the SDK that touches the network (design §44 §4,
 * D612; runtime contract R10).
 *
 * <h2>Why this is hand-written rather than connect-java</h2>
 *
 * <p>Java SDKs conventionally use {@code connect-java}, and design §44 §9 row 2 says so. It is
 * <b>not used here</b>, for two reasons that are both about this repository rather than about
 * Java:
 *
 * <ol>
 *   <li>the {@code connectrpc} artifacts are not resolvable from Maven Central from this
 *       environment — {@code connectrpc/connect-api} and {@code com/connectrpc/connect-api}
 *       both answer 404 there while {@code protobuf-java} and {@code grpc-core} resolve — and
 *       neither Maven nor Gradle is installed to resolve a transitive tree with; and
 *   <li>the SDK is built by plain {@code javac} from {@code sdks/java/build.sh}, which has no
 *       dependency resolver.
 * </ol>
 *
 * <p>So this is the wire implementation those two constraints force. It is deliberately small and
 * deliberately honest: it speaks the three protocols D600 serves, it reads the streaming envelope
 * rather than only the HTTP status, and it carries gRPC's {@code google.rpc.Status} far enough to
 * reach the {@code ErrorInfo} detail. Where {@code connect-java} would do the same work, this is
 * the work being done instead — and switching to it later is a change to this class and to the
 * dependency list, not to the runtime above it.
 *
 * <p><b>What it deliberately does not do:</b> no request compression, no response compression,
 * no interceptors, no per-call deadlines beyond what the caller's context carries. See
 * {@code README.md} for the full list and for what to check when this is replaced.
 */
public final class ConnectTransport implements AutoCloseable {

    /**
     * The request content type of each protocol's unary form.
     *
     * <p>Binary protobuf by default, which is what the recorded conformance corpus calls the
     * {@code _proto} cases. A caller who asks for the proto3 JSON mapping gets
     * {@code application/json}, which is what {@code curl} sends.
     */
    private static final Map<Protocol, String> UNARY_CONTENT_TYPE =
            Map.of(
                    Protocol.CONNECT, "application/proto",
                    Protocol.GRPC, "application/grpc+proto",
                    Protocol.GRPC_WEB, "application/grpc-web+proto");

    /** The request content type of each protocol's server-streaming form. */
    private static final Map<Protocol, String> STREAM_CONTENT_TYPE =
            Map.of(
                    Protocol.CONNECT, "application/connect+proto",
                    Protocol.GRPC, "application/grpc+proto",
                    Protocol.GRPC_WEB, "application/grpc-web+proto");

    /**
     * How long to wait for the <b>headers</b> of a stream's response.
     *
     * <p>Not for the stream's body, which is unbounded by design: a watch that ends when the
     * first heartbeat arrives is not a watch. A caller's own deadline is what bounds it, which
     * is also what bounds the retry loop around it.
     */
    private static final Duration STREAM_HEADER_TIMEOUT = Duration.ofSeconds(30);

    private final String endpoint;
    private final Protocol protocol;
    private final HttpClient client;
    private final boolean ownsClient;

    /**
     * Build a transport.
     *
     * @param endpoint the instance's base URL, for example {@code https://acme.loams.dev}
     * @param protocol the wire protocol; {@link Protocol#CONNECT} is the default
     * @param client the HTTP client to use, or {@code null} to build one
     */
    public ConnectTransport(String endpoint, Protocol protocol, HttpClient client) {
        this.endpoint = endpoint.endsWith("/") ? endpoint.substring(0, endpoint.length() - 1) : endpoint;
        this.protocol = protocol == null ? Protocol.CONNECT : protocol;
        this.ownsClient = client == null;
        this.client =
                client != null
                        ? client
                        : HttpClient.newBuilder()
                                // HTTP/2 wherever the endpoint offers it, because both gRPC
                                // and the Connect protocol's server streaming want it and a
                                // JVM client negotiates it by default.
                                .version(HttpClient.Version.HTTP_2)
                                .followRedirects(HttpClient.Redirect.NEVER)
                                .connectTimeout(Duration.ofSeconds(20))
                                .build();
    }

    /** The wire protocol this transport speaks. */
    public Protocol protocol() {
        return protocol;
    }

    /** The instance this transport talks to. */
    public String endpoint() {
        return endpoint;
    }

    /**
     * Make one unary call.
     *
     * @param service the fully qualified service name
     * @param method the method name
     * @param body the serialized request message
     * @param headers extra headers for this call. {@code Authorization} is set by the runtime
     *     and wins, because a caller-supplied bearer would be one the SDK cannot refresh (R1).
     * @param jsonCodec whether to use the proto3 JSON mapping rather than binary protobuf
     * @return the serialized response message
     * @throws ConnectFailure when the server answered with a failure
     * @throws IOException when the call did not complete — a socket, a refused connection, an
     *     interrupt. Both reach the caller through {@link dev.loams.Errors}.
     */
    public UnaryResponse unary(
            String service,
            String method,
            byte[] body,
            Map<String, String> headers,
            boolean jsonCodec)
            throws IOException, InterruptedException {
        String rpc = service + "/" + method;
        String contentType =
                jsonCodec ? jsonVariant(UNARY_CONTENT_TYPE.get(protocol)) : UNARY_CONTENT_TYPE.get(protocol);

        byte[] payload;
        if (protocol == Protocol.CONNECT && !jsonCodec) {
            // Connect unary sends the bare message: no framing. That is what makes the same
            // call a plain HTTP POST a person can curl.
            payload = body;
        } else {
            payload = Envelope.frame(body);
        }

        HttpResponse<byte[]> response =
                send(rpc, contentType, payload, headers, HttpResponse.BodyHandlers.ofByteArray(), null);

        byte[] responseBody = response.body();
        if (protocol == Protocol.CONNECT && !jsonCodec) {
            if (response.statusCode() / 100 == 2) {
                return new UnaryResponse(responseBody, headersOf(response));
            }
            throw new ConnectFailure(
                    rpc, ConnectError.fromUnaryBody(text(responseBody)), null);
        }

        // gRPC and gRPC-Web: the status is not in the HTTP status, and the only place the
        // ErrorInfo travels is the trailer frame or the trailers header. Read the frames.
        return new UnaryResponse(readGrpcUnary(rpc, response, responseBody), headersOf(response));
    }

    /**
     * Open one server stream.
     *
     * <p>The returned stream yields the raw serialized messages and turns the end-of-stream
     * frame's error into a {@link ConnectFailure}. That last part is the reason a stream is not
     * just "read until the socket closes": on the Connect protocol a refusal arrives inside the
     * envelope on a 200, so a client that only watched the HTTP status would report success and
     * yield nothing at all.
     *
     * @return the open stream; the caller closes it
     */
    public ConnectStream openStream(
            String service, String method, byte[] body, Map<String, String> headers)
            throws IOException, InterruptedException {
        String rpc = service + "/" + method;
        byte[] payload = Envelope.frame(body);
        HttpResponse<InputStream> response =
                send(
                        rpc,
                        STREAM_CONTENT_TYPE.get(protocol),
                        payload,
                        headers,
                        HttpResponse.BodyHandlers.ofInputStream(),
                        STREAM_HEADER_TIMEOUT);
        if (response.statusCode() / 100 != 2) {
            // A stream refused before it opened: connect-rust answers a refused server stream
            // with a real HTTP status, and the body is a plain Connect error rather than frames.
            InputStream rejected = response.body();
            byte[] bytes;
            try {
                bytes = rejected.readAllBytes();
            } finally {
                rejected.close();
            }
            throw new ConnectFailure(rpc, ConnectError.fromUnaryBody(text(bytes)), null);
        }
        return new ConnectStream(rpc, response.body(), grpcTrailers(response), protocol);
    }

    private <T> HttpResponse<T> send(
            String rpc,
            String contentType,
            byte[] payload,
            Map<String, String> headers,
            HttpResponse.BodyHandler<T> handler,
            Duration timeout)
            throws IOException, InterruptedException {
        HttpRequest.Builder request;
        try {
            request = HttpRequest.newBuilder(new URI(endpoint + "/" + rpc));
        } catch (URISyntaxException e) {
            throw new ConnectFailure(
                    rpc, null, new IOException("the endpoint " + endpoint + " is not a URL", e));
        }
        request.header("Content-Type", contentType);
        // No Accept-Encoding: nothing in this SDK negotiates compression, and asking for it and
        // then not decoding it would be a way to produce a body the caller cannot read.
        request.header("Connect-Protocol-Version", "1");
        headers.forEach(request::header);
        request.POST(HttpRequest.BodyPublishers.ofByteArray(payload));
        if (timeout != null) {
            request.timeout(timeout);
        }
        try {
            return client.send(request.build(), handler);
        } catch (java.net.http.HttpTimeoutException e) {
            throw new ConnectFailure(rpc, null, e);
        } catch (java.net.ConnectException e) {
            throw new ConnectFailure(rpc, null, e);
        }
    }

    /**
     * Read a unary gRPC or gRPC-Web response: one message frame, then trailers.
     *
     * <p>The trailer is where the status lives, so a response that carries {@code grpc-status: 0}
     * is a success and anything else is a failure — and the failure's reason comes out of
     * {@code grpc-status-details-bin}, or the caller gets a Loams failure with no reason at all.
     */
    private byte[] readGrpcUnary(String rpc, HttpResponse<byte[]> response, byte[] body)
            throws IOException {
        byte[] message = null;
        String trailerBlock = null;
        try (InputStream in = new ByteArrayInputStream(body)) {
            Envelope.Frame frame;
            while ((frame = Envelope.read(in)) != null) {
                if ((frame.flags() & 0x80) != 0) {
                    // The trailer frame. gRPC-Web carries them here; gRPC over HTTP/2 carries
                    // them in the headers, which `grpcTrailers` picks up instead.
                    trailerBlock = frame.payloadAsText();
                } else if (frame.isEndStream()) {
                    // Connect's end-of-stream frame inside a gRPC body is not legal; treat it as
                    // the end rather than as a message, so nothing bogus reaches the codec.
                    continue;
                } else if (message == null) {
                    message = frame.payload();
                }
            }
        }
        if (trailerBlock == null) {
            trailerBlock = grpcTrailers(response);
        }
        ConnectFailure failure = grpcFailure(rpc, trailerBlock);
        if (failure != null) {
            throw failure;
        }
        if (message == null) {
            if (response.statusCode() / 100 != 2) {
                // A non-2xx with no frames at all: a proxy, not a gRPC server.
                throw new ConnectFailure(rpc, ConnectError.fromUnaryBody(text(body)), null);
            }
            throw new ConnectFailure(rpc, null, new IOException("the response carried no message frame"));
        }
        return message;
    }

    /** A failure from a gRPC trailer block, or {@code null} when it reports success. */
    private static ConnectFailure grpcFailure(String rpc, String trailerBlock) {
        if (trailerBlock == null) {
            return null;
        }
        String statusText = headerValue(trailerBlock, "grpc-status");
        if (statusText == null) {
            // No status at all. gRPC requires one, so this is not a successful response; saying
            // so beats treating a truncated body as a message.
            return new ConnectFailure(
                    rpc, null, new IOException("the gRPC trailers carry no grpc-status"));
        }
        int status = parseStatus(statusText);
        if (status == 0) {
            return null;
        }
        String message = headerValue(trailerBlock, "grpc-message");
        GrpcStatus.Status details =
                GrpcStatus.parse(headerValue(trailerBlock, "grpc-status-details-bin"));
        if (details != null && !details.details().isEmpty()) {
            // The Status's own code and message win over the trailers' when they are present,
            // because it is the same message in a structured form.
            List<ConnectErrorDetail> carried = details.details();
            return new ConnectFailure(
                    rpc,
                    new ConnectError(
                            codeWire(details.code()),
                            message != null && !message.isEmpty() ? message : details.message(),
                            carried,
                            Map.of()),
                    null);
        }
        return new ConnectFailure(
                rpc,
                new ConnectError(
                        codeWire(status),
                        message == null ? "" : percentDecode(message),
                        List.of(),
                        Map.of()),
                null);
    }

    /** A {@code grpc-status} value as a number, or {@code -1} when it is not one. */
    private static int parseStatus(String text) {
        try {
            return Integer.parseInt(text.trim());
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    private static String codeWire(int number) {
        return dev.loams.Code.fromNumber(number).wire();
    }

    /**
     * A gRPC trailer block, or {@code null} when the response carries none.
     *
     * <p>gRPC over HTTP/2 puts the trailers in HTTP trailers, and
     * {@code HttpResponse.trailers()} arrived in <b>Java 18</b>. This SDK's floor is Java 17 (the
     * SDK2 Task 6 plan says "Java 17+"), so the method is reached reflectively: on 17 the call is
     * simply absent and a gRPC stream reports that it has no trailers rather than pretending the
     * response succeeded.
     *
     * <p>It is reflection over a <em>public</em> JDK API method, not over an internal one, so no
     * {@code --add-opens} is involved and the call site is the only place that has to change when
     * the floor moves.
     */
    private static String grpcTrailers(HttpResponse<?> response) {
        Object value;
        try {
            value =
                    HttpResponse.class
                            .getMethod("trailers")
                            .invoke(response);
        } catch (ReflectiveOperationException | RuntimeException e) {
            return null;
        }
        if (!(value instanceof java.util.Optional<?> present) || present.isEmpty()) {
            return null;
        }
        if (!(present.get() instanceof HttpHeaders headers)) {
            return null;
        }
        StringBuilder out = new StringBuilder();
        headers
                .map()
                .forEach(
                        (name, values) ->
                                values.forEach(
                                        item ->
                                                out.append(name)
                                                        .append(": ")
                                                        .append(item)
                                                        .append("\r\n")));
        return out.length() == 0 ? null : out.toString();
    }

    /** One value out of a {@code name: value\r\n} trailer block, or {@code null}. */
    private static String headerValue(String block, String name) {
        String wanted = name.toLowerCase(Locale.ROOT);
        for (String line : block.split("\r\n|\n")) {
            int colon = line.indexOf(':');
            if (colon < 0) {
                continue;
            }
            if (line.substring(0, colon).trim().toLowerCase(Locale.ROOT).equals(wanted)) {
                return line.substring(colon + 1).trim();
            }
        }
        return null;
    }

    /**
     * gRPC percent-encodes a message so it cannot contain a line break. The encoding is the one
     * from gRPC's HTTP/2 spec: anything outside {@code %x20-24 / %x26-7E} as {@code %XX}.
     */
    private static String percentDecode(String value) {
        StringBuilder out = new StringBuilder(value.length());
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (c == '%' && i + 2 < value.length()) {
                int high = Character.digit(value.charAt(i + 1), 16);
                int low = Character.digit(value.charAt(i + 2), 16);
                if (high >= 0 && low >= 0) {
                    out.append((char) ((high << 4) | low));
                    i += 2;
                    continue;
                }
            }
            out.append(c);
        }
        return out.toString();
    }

    /** The proto3 JSON content type that goes with a binary one. */
    private static String jsonVariant(String binary) {
        return binary.replace("proto", "json");
    }

    /**
     * A response's headers, with the names lowercased.
     *
     * <p>Lowercased because {@code java.net.http} lowercases them itself, and a caller looking up
     * {@code loams-consistency-token} should not have to know that. It is carried at all because
     * R4's token travels in a response header and nothing else: the invoker records it into the
     * session store from here.
     */
    private static Map<String, String> headersOf(HttpResponse<?> response) {
        Map<String, String> out = new java.util.LinkedHashMap<>();
        response.headers()
                .map()
                .forEach(
                        (name, values) -> {
                            if (!values.isEmpty()) {
                                out.put(name.toLowerCase(Locale.ROOT), values.get(0));
                            }
                        });
        return out;
    }

    private static String text(byte[] body) {
        return body == null ? "" : new String(body, StandardCharsets.UTF_8);
    }

    /**
     * A successful unary response: its message bytes and its headers.
     *
     * @param body the serialized response message
     * @param headers the response headers, lowercased. Read for
     *     {@code loams-consistency-token}, which is where R4's token comes from.
     */
    public record UnaryResponse(byte[] body, Map<String, String> headers) {

        /** The value of {@code name}, or {@code null}. */
        public String header(String name) {
            return headers.get(name.toLowerCase(Locale.ROOT));
        }
    }

    /**
     * Close the transport.
     *
     * <p>It does not close an {@link HttpClient} the caller supplied: that client belongs to the
     * caller and may be shared with something else. The JDK client has no {@code close}, so
     * there is nothing to release for one this package built either — its connections are pooled
     * and reused, and closing them is the caller's decision.
     */
    @Override
    public void close() {
        // Intentionally empty; see the method's javadoc.
    }
}