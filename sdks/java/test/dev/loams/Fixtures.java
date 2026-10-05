package dev.loams;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import dev.loams.internal.Json;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Base64;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.TimeUnit;

/**
 * The conformance fixture server, from a Java test (design §44 §10.4, SDK1 Task 4).
 *
 * <p>The corpus in {@code sdks/fixtures} is recorded from a real {@code loams dev}, and every
 * SDK's suite replays it so thirteen clients can be compared against the same bytes. This is how a
 * Java test gets an endpoint.
 *
 * <p>Three ways to get one, in the order they are tried — the same order, and for the same
 * reasons, as {@code sdks/go/fixture_server_test.go}:
 *
 * <ol>
 *   <li>{@code LOAMS_TEST_ENDPOINT} — a live {@code loams dev}. This short-circuits everything
 *       else, which is how {@code sdks/conformance/run.sh} runs the same suite in CI against a
 *       recording and on a developer machine against the real thing.
 *   <li>{@code node sdks/conformance/fixture-server.mjs} — the shared server. Preferred when
 *       Node is on PATH, because then the Java suite really does run against the same server the
 *       other twelve do.
 *   <li>An in-process replay of {@code sdks/fixtures/recorded} with the same matching rules, for
 *       a machine with no Node. A JVM test suite should not need a JavaScript runtime to run, and
 *       the corpus is the part that matters: the recorded bytes are identical either way.
 * </ol>
 *
 * <p>What all three agree on is the corpus. {@link ConformanceTest} reads
 * {@code sdks/fixtures/index.json} and fails if a case is missing, so a suite cannot quietly stop
 * covering something.
 */
final class Fixtures implements AutoCloseable {

    private static final String FIXTURE_SERVER_JS = "sdks/conformance/fixture-server.mjs";

    private final String endpoint;

    /** Non-null when this suite started a process it has to stop. */
    private final Process process;

    /** Non-null when this suite started an in-process server. */
    private final HttpServer server;

    private Fixtures(String endpoint, Process process, HttpServer server) {
        this.endpoint = endpoint;
        this.process = process;
        this.server = server;
    }

    /** The base URL the client is built with. */
    String endpoint() {
        return endpoint;
    }

    /**
     * Bring up whichever of the three endpoints is available.
     *
     * @throws AssertionError when none of them could be started, which is a failure rather than a
     *     skip: a suite that quietly ran against nothing would be worse than no suite
     */
    static Fixtures start() {
        String live = System.getenv("LOAMS_TEST_ENDPOINT");
        if (live != null && !live.isBlank()) {
            return new Fixtures(stripTrailingSlash(live), null, null);
        }
        if (onPath("node")) {
            Fixtures started = startNodeFixtureServer();
            if (started != null) {
                return started;
            }
        }
        return startReplayServer();
    }

    private static Fixtures startNodeFixtureServer() {
        Path root = repositoryRoot();
        Path server = root.resolve(FIXTURE_SERVER_JS);
        if (!Files.isRegularFile(server)) {
            return null;
        }
        Process started;
        try {
            started =
                    new ProcessBuilder(
                                    "node",
                                    server.toString(),
                                    "--fixtures",
                                    root.resolve("sdks/fixtures").toString(),
                                    "--port",
                                    "0")
                            .redirectError(ProcessBuilder.Redirect.INHERIT)
                            .start();
        } catch (IOException e) {
            return null;
        }
        // The server prints `{"url":"http://127.0.0.1:PORT"}` on stdout once it is listening.
        // Reading it with a watchdog rather than blocking forever is what keeps a server that
        // dies on startup from hanging the suite instead of failing it.
        try (InputStream out = started.getInputStream()) {
            long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(30);
            StringBuilder line = new StringBuilder();
            while (System.nanoTime() < deadline) {
                int b = out.read();
                if (b < 0) {
                    break;
                }
                if (b == '\n') {
                    String parsed = line.toString().trim();
                    line.setLength(0);
                    if (parsed.isEmpty()) {
                        continue;
                    }
                    String url = readUrl(parsed);
                    if (url != null) {
                        return new Fixtures(stripTrailingSlash(url), started, null);
                    }
                } else {
                    line.append((char) b);
                }
                if (out.available() == 0) {
                    // Nothing buffered: yield rather than spin a core waiting for the next byte.
                    Thread.sleep(10);
                }
            }
        } catch (IOException | InterruptedException e) {
            if (e instanceof InterruptedException) {
                Thread.currentThread().interrupt();
            }
        }
        started.destroyForcibly();
        return null;
    }

    private static String readUrl(String line) {
        try {
            return Json.string(Json.parseObject(line), "url");
        } catch (RuntimeException e) {
            // Not the announcement line; the server logs other things to stdout.
            return null;
        }
    }

    /**
     * Serve {@code sdks/fixtures/recorded} over the JDK's own HTTP server, with the same
     * matching rules as {@code fixture-server.mjs}: keyed on method, path and content
     * <em>family</em>, and a <b>404 naming the gap</b> for anything unrecorded rather than a silent
     * success.
     */
    private static Fixtures startReplayServer() {
        Path root = repositoryRoot();
        Path recorded = root.resolve("sdks/fixtures/recorded");
        List<Path> files;
        try (var stream = Files.list(recorded)) {
            files = stream.filter(p -> p.toString().endsWith(".json")).toList();
        } catch (IOException e) {
            throw new AssertionError("no recorded fixtures under " + recorded + ": " + e, e);
        }
        if (files.isEmpty()) {
            throw new AssertionError("no recorded fixtures under " + recorded);
        }

        Map<String, Recorded> cases = new LinkedHashMap<>();
        List<String> keys = new ArrayList<>();
        for (Path file : files) {
            Recorded entry = Recorded.parse(file);
            String key = fixtureKey(entry.method, entry.path, entry.requestContentType);
            cases.put(key, entry);
            keys.add(key);
        }

        HttpServer http;
        try {
            http = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        } catch (IOException e) {
            throw new AssertionError("could not start the in-process fixture replay: " + e, e);
        }
        http.createContext(
                "/",
                exchange -> {
                    String key =
                            fixtureKey(
                                    exchange.getRequestMethod(),
                                    exchange.getRequestURI().getPath(),
                                    exchange.getRequestHeaders().getFirst("content-type"));
                    Recorded entry = cases.get(key);
                    if (entry == null) {
                        // Loudly. A suite that passed because everything answered 200 would be
                        // worse than no suite.
                        respond(
                                exchange,
                                404,
                                "application/json",
                                ("{\"error\":\"no recorded fixture for "
                                                + key
                                                + "\",\"recorded\":"
                                                + jsonArray(keys)
                                                + "}")
                                        .getBytes(StandardCharsets.UTF_8));
                        return;
                    }
                    // The request is drained rather than ignored: an unread request body is what
                    // makes the JDK server close the connection instead of answering.
                    drain(exchange);
                    for (Map.Entry<String, String> header : entry.responseHeaders.entrySet()) {
                        exchange.getResponseHeaders().set(header.getKey(), header.getValue());
                    }
                    exchange.sendResponseHeaders(entry.status, entry.responseBody.length);
                    exchange.getResponseBody().write(entry.responseBody);
                    exchange.close();
                });
        http.setExecutor(null);
        http.start();
        String url = "http://127.0.0.1:" + http.getAddress().getPort();
        return new Fixtures(url, null, http);
    }

    private static void respond(HttpExchange exchange, int status, String contentType, byte[] body)
            throws IOException {
        exchange.getResponseHeaders().set("content-type", contentType);
        exchange.sendResponseHeaders(status, body.length);
        exchange.getResponseBody().write(body);
        exchange.close();
    }

    private static void drain(HttpExchange exchange) {
        try (InputStream in = exchange.getRequestBody()) {
            readAll(in);
        } catch (IOException e) {
            // Nothing useful to do; the response is what the suite reads.
        }
    }

    private static byte[] readAll(InputStream in) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        byte[] buffer = new byte[8192];
        int n;
        while ((n = in.read(buffer)) > 0) {
            out.write(buffer, 0, n);
        }
        return out.toByteArray();
    }

    private static String jsonArray(List<String> values) {
        return values.stream()
                .map(value -> "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\"")
                .reduce((a, b) -> a + "," + b)
                .map(body -> "[" + body + "]")
                .orElse("[]");
    }

    /**
     * How a recorded case is matched: method, path and the <em>family</em> of the content type.
     *
     * <p>The family rather than the exact string because a client's transport picks one encoding
     * and the corpus carries all four; naming the families keeps the in-process replay and
     * {@code fixture-server.mjs} in step.
     */
    static String fixtureKey(String method, String path, String contentType) {
        return method + " " + path + " " + contentFamily(contentType);
    }

    /** Mirrors {@code family()} in {@code sdks/conformance/fixture-server.mjs}. */
    static String contentFamily(String contentType) {
        if (contentType == null) {
            return "none";
        }
        String base = contentType;
        int semicolon = base.indexOf(';');
        if (semicolon >= 0) {
            base = base.substring(0, semicolon);
        }
        base = base.trim();
        if (base.equals("application/grpc-web+json")) {
            return "grpc_web_json";
        }
        if (base.startsWith("application/grpc-web")) {
            return "grpc_web";
        }
        if (base.startsWith("application/connect")) {
            return "connect";
        }
        if (base.equals("application/json")) {
            return "json";
        }
        if (base.equals("application/proto")) {
            return "proto";
        }
        if (base.isEmpty()) {
            return "none";
        }
        return base;
    }

    /**
     * A syntactically valid endpoint that nothing is listening on.
     *
     * <p>For a test that builds a client and asserts on its configuration without making a call.
     * Port 1 is privileged, so nothing is bound to it and nothing will answer.
     */
    static String fixtureServerEndpoint() {
        return "http://127.0.0.1:1";
    }

    /** The repository root, found by walking up from the working directory. */
    static Path repositoryRoot() {
        Path here = Path.of("").toAbsolutePath();
        for (Path candidate = here; candidate != null; candidate = candidate.getParent()) {
            if (Files.isDirectory(candidate.resolve("sdks/fixtures"))
                    && Files.isDirectory(candidate.resolve("proto"))) {
                return candidate;
            }
        }
        throw new AssertionError(
                "could not find the repository root from " + here + ": expected sdks/fixtures and proto");
    }

    /** The corpus index, {@code sdks/fixtures/index.json}. */
    static Map<String, Object> corpusIndex() {
        Path index = repositoryRoot().resolve("sdks/fixtures/index.json");
        try {
            return Json.parseObject(Files.readString(index));
        } catch (IOException | RuntimeException e) {
            throw new AssertionError("reading the fixture corpus index: " + e, e);
        }
    }

    private static boolean onPath(String program) {
        String path = System.getenv("PATH");
        if (path == null) {
            return false;
        }
        for (String entry : path.split(File.pathSeparator)) {
            if (Files.isExecutable(Path.of(entry, program))) {
                return true;
            }
        }
        return false;
    }

    private static String stripTrailingSlash(String value) {
        return value.endsWith("/") ? value.substring(0, value.length() - 1) : value;
    }

    /** Stop whatever this suite started. */
    @Override
    public void close() {
        if (server != null) {
            server.stop(0);
        }
        if (process != null) {
            process.destroy();
            try {
                if (!process.waitFor(5, TimeUnit.SECONDS)) {
                    process.destroyForcibly();
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                process.destroyForcibly();
            }
        }
    }

    /** One recorded case, as the corpus stores it. */
    static final class Recorded {

        final String name;
        final String method;
        final String path;
        final String requestContentType;
        final int status;
        final Map<String, String> responseHeaders;
        final byte[] responseBody;

        private Recorded(
                String name,
                String method,
                String path,
                String requestContentType,
                int status,
                Map<String, String> responseHeaders,
                byte[] responseBody) {
            this.name = name;
            this.method = method;
            this.path = path;
            this.requestContentType = requestContentType;
            this.status = status;
            this.responseHeaders = responseHeaders;
            this.responseBody = responseBody;
        }

        static Recorded parse(Path file) {
            try {
                Map<String, Object> root = Json.parseObject(Files.readString(file));
                Map<String, Object> request = Json.object(root, "request");
                Map<String, Object> response = Json.object(root, "response");
                if (request == null || response == null) {
                    throw new IllegalArgumentException(file + " has no request/response");
                }
                Map<String, Object> headers = Json.object(request, "headers");
                return new Recorded(
                        Json.string(root, "name", file.getFileName().toString()),
                        Json.string(request, "method", "POST"),
                        Json.string(request, "path", "/"),
                        headers == null ? "" : Json.string(headers, "content-type", ""),
                        (int) doubleOrZero(response.get("status")),
                        stringMap(Json.object(response, "headers")),
                        bodyOf(response));
            } catch (IOException | RuntimeException e) {
                throw new AssertionError("parsing " + file + ": " + e, e);
            }
        }

        private static double doubleOrZero(Object value) {
            return value instanceof Double number ? number : 0d;
        }

        private static Map<String, String> stringMap(Map<String, Object> source) {
            Map<String, String> out = new LinkedHashMap<>();
            if (source != null) {
                source.forEach(
                        (name, value) -> {
                            if (value instanceof String text) {
                                out.put(name.toLowerCase(Locale.ROOT), text);
                            }
                        });
            }
            return out;
        }

        /**
         * A case's body: the JSON body, or the raw bytes of a {@code bodyBase64} one. Exactly one
         * of the two is present in every recorded case.
         */
        private static byte[] bodyOf(Map<String, Object> response) throws IOException {
            String base64 = Json.string(response, "bodyBase64");
            if (base64 != null && !base64.isEmpty()) {
                return Base64.getDecoder().decode(base64);
            }
            Object body = response.get("body");
            if (body instanceof String text) {
                return text.getBytes(StandardCharsets.UTF_8);
            }
            return new byte[0];
        }
    }

    /**
     * The recorded response body of one corpus case, base64-decoded.
     *
     * <p>It is how a test replays bytes rather than inventing them, so the message the SDK parses
     * in a unit test is the message the corpus recorded and every other SDK's suite parses.
     *
     * @param caseName the case's name, without the {@code .json} suffix
     */
    static byte[] readRecordedBody(String caseName) {
        Path file = repositoryRoot().resolve("sdks/fixtures/recorded").resolve(caseName + ".json");
        try {
            return Recorded.parse(file).responseBody;
        } catch (AssertionError e) {
            throw e;
        } catch (RuntimeException e) {
            throw new AssertionError("reading the recorded body of " + caseName + ": " + e, e);
        }
    }

    /** An unused port, for a test that needs one. */
    static int freePort() {
        try (ServerSocket socket = new ServerSocket(0)) {
            return socket.getLocalPort();
        } catch (IOException e) {
            throw new AssertionError("no free port: " + e, e);
        }
    }

    /** Parse a base64 string, for a test that replays recorded bytes. */
    static byte[] base64(String value) {
        return Base64.getDecoder().decode(value);
    }

    /** The URI of an endpoint path, for a diagnostic message. */
    static URI uri(String endpoint, String path) {
        return URI.create(endpoint + path);
    }
}