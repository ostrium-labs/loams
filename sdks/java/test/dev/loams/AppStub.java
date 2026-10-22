package dev.loams;

import com.google.protobuf.Message;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * A scripted Connect server for the app tests: each request takes the next reply (the last one
 * repeats), and every request is recorded as path, headers and body.
 */
final class AppStub implements AutoCloseable {

    record Request(String path, Map<String, List<String>> headers, byte[] body) {
        String header(String name) {
            for (var entry : headers.entrySet()) {
                if (entry.getKey().equalsIgnoreCase(name)) {
                    return entry.getValue().get(0);
                }
            }
            return null;
        }
    }

    record Reply(int status, byte[] body, String contentType, Map<String, String> headers, long delayMs) {}

    static Reply ok(Message message) {
        return new Reply(200, message.toByteArray(), "application/proto", Map.of(), 0);
    }

    static Reply ok(Message message, Map<String, String> headers) {
        return new Reply(200, message.toByteArray(), "application/proto", headers, 0);
    }

    static Reply refusal(int status, String code, String message) {
        String json = "{\"code\":\"" + code + "\",\"message\":\"" + message + "\"}";
        return new Reply(status, json.getBytes(StandardCharsets.UTF_8), "application/json", Map.of(), 0);
    }

    static Reply hang(long delayMs) {
        return new Reply(200, new byte[0], "application/proto", Map.of(), delayMs);
    }

    private final HttpServer server;
    private final List<Reply> replies;
    final List<Request> requests = new CopyOnWriteArrayList<>();
    private final AtomicInteger next = new AtomicInteger();

    AppStub(Reply... replies) throws IOException {
        this.replies = new ArrayList<>(List.of(replies));
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            byte[] body = exchange.getRequestBody().readAllBytes();
            requests.add(new Request(exchange.getRequestURI().getPath(), Map.copyOf(exchange.getRequestHeaders()), body));
            int index = Math.min(next.getAndIncrement(), this.replies.size() - 1);
            Reply reply = this.replies.get(index);
            if (reply.delayMs() > 0) {
                try {
                    Thread.sleep(reply.delayMs());
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
            exchange.getResponseHeaders().set("content-type", reply.contentType());
            reply.headers().forEach(exchange.getResponseHeaders()::set);
            try {
                exchange.sendResponseHeaders(reply.status(), reply.body().length == 0 ? -1 : reply.body().length);
                if (reply.body().length > 0) {
                    exchange.getResponseBody().write(reply.body());
                }
            } catch (IOException ignored) {
                // The client gave up (a deadline test); nothing to answer.
            }
            exchange.close();
        });
        server.setExecutor(java.util.concurrent.Executors.newCachedThreadPool());
        server.start();
    }

    String endpoint() {
        return "http://127.0.0.1:" + server.getAddress().getPort();
    }

    Client client(String namespace, boolean session) {
        return Client.of(Options.builder()
                .endpoint(endpoint())
                .namespace(namespace)
                .sessionConsistency(session)
                .maxRetries(3)
                .build());
    }

    @Override
    public void close() {
        server.stop(0);
    }
}
