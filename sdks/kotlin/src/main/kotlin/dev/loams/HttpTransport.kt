package dev.loams

import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.DynamicMessage
import java.io.IOException
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.time.Duration

/**
 * The shipped HTTP transport, over `java.net.http.HttpClient`.
 *
 * ## Why `java.net.http` and not OkHttp
 *
 * It is in the JDK, it speaks HTTP/1.1 and HTTP/2, and it is what a JVM application
 * already has. An SDK that pulled in OkHttp to make six POSTs would add a dependency
 * to every consumer's dependency tree — on Android, where OkHttp is *already* there and
 * a second copy of it is a real cost.
 *
 * ## Why the transport is an interface anyway
 *
 * Because `java.net.http` is a reasonable default and an unreasonable requirement: an
 * Android app has OkHttp and a coroutine dispatcher, and forcing `java.net.http` onto
 * it would fail on API levels that lack it. So this is **one class** implementing
 * [Transport], and substituting it is an interface implementation rather than a fork.
 * See `DEPENDENCIES.md` for the full table of what is and is not here.
 *
 * ## What it does not do
 *
 * **No compression.** Nothing negotiates it and the transport does not send
 * `Accept-Encoding: gzip` — asking for compression and then not decoding it is a way to
 * produce an unreadable body. A server that compresses anyway is refused rather than
 * handed to a parser that would produce nonsense.
 *
 * **No HTTP trailers.** gRPC proper carries its status in HTTP trailers, which HTTP/1.1
 * cannot and which this SDK does not speak: the corpus records gRPC-Web, whose trailers
 * are in the body, and Connect, whose end frame is in the body. A caller needing gRPC
 * proper supplies a [Transport] that can read them.
 */
class HttpTransport(
    /** The base URL, for example `https://api.loams.dev`. */
    endpoint: String,
    /** How long one attempt may take. Every outbound call needs a bound. */
    private val timeout: Duration = Duration.ofSeconds(30),
    /** Extra headers on every call, for a proxy or a gateway that needs them. */
    private val defaultHeaders: Map<String, String> = emptyMap(),
    /** The client to use. A caller with its own pool, executor or dispatcher passes one. */
    client: HttpClient? = null,
) : Transport {
    private val base: String = endpoint.trimEnd('/')

    private val client: HttpClient = client ?: HttpClient.newBuilder()
        .version(HttpClient.Version.HTTP_1_1)
        // Connect streaming is a chunked response over HTTP/1.1, and a server that
        // buffers it behind an HTTP/2 flow-control window would defeat the point of a
        // stream. HTTP/1.1 is therefore the default rather than a preference.
        .connectTimeout(timeout)
        .build()

    init {
        require(endpoint.isNotBlank()) {
            "the endpoint is empty; a Loams client needs a base URL such as https://api.loams.dev"
        }
        require(endpoint.startsWith("http://") || endpoint.startsWith("https://")) {
            "the endpoint '$endpoint' is not an http or https URL"
        }
    }

    override fun send(request: TransportRequest): TransportResponse {
        val builder = HttpRequest.newBuilder(URI.create(base + request.path))
            .timeout(timeout)
            // `POST` always: Connect unary is a POST and the framing protocols are too,
            // and a GET would need the idempotency level's blessing for a body.
            .POST(HttpRequest.BodyPublishers.ofByteArray(request.body))

        for ((name, value) in defaultHeaders) {
            builder.header(name, value)
        }
        for ((name, value) in request.headers) {
            // `Content-Type` is set through `header`, not `setHeader`, because a caller
            // may legitimately pass it in `headers` and the last one written has to win
            // consistently with how `bodyPublisher` was chosen.
            builder.header(name, value)
        }
        builder.header("Content-Type", request.contentType)

        val response = try {
            client.send(builder.build(), HttpResponse.BodyHandlers.ofByteArray())
        } catch (interrupted: InterruptedException) {
            // Restores the flag so a caller above can see it, and reports the
            // cancellation as a cancellation rather than as a transport failure.
            Thread.currentThread().interrupt()
            throw java.util.concurrent.CancellationException("$request.rpc: the call was interrupted")
        } catch (io: IOException) {
            throw io
        }

        val headers = LinkedHashMap<String, String>()
        response.headers().map().forEach { (name, values) ->
            // Last value wins, which is what HTTP says for a repeated header and what
            // `grpc-message` split across folds needs.
            values.lastOrNull()?.let { headers[name] = it }
        }
        val contentType = response.headers().firstValue("content-type").orElse("")

        // A server that answered `Content-Encoding: gzip` when the client never asked is
        // refused, because the bytes below are compressed and every parser here would
        // report a malformed message rather than the real answer.
        val encoding = headers.entries.firstOrNull { it.key.equals("Content-Encoding", ignoreCase = true) }?.value
        if (!encoding.isNullOrEmpty() && !encoding.equals("identity", ignoreCase = true)) {
            throw ErrorMapper.internal(
                request.rpc,
                "the server answered Content-Encoding: $encoding, and this SDK asks for no compression and " +
                    "cannot read one",
            )
        }

        return TransportResponse(response.statusCode(), headers, contentType, response.body())
    }
}

/**
 * The transport a client sends over, when the caller has none.
 *
 * An endpoint the SDK was not given is a **refusal at construction** rather than a
 * failure on the first call: a client with no endpoint has nothing to do, and finding
 * that out when a call is made means the failure is reported against whichever RPC
 * happened to be first.
 */
internal fun resolveTransport(endpoint: String?, supplied: Transport?): Transport {
    if (supplied != null) {
        return supplied
    }
    val endpoint = endpoint ?: throw IllegalArgumentException(
        "a Loams client needs either an endpoint or a transport; neither was given, so there is nothing to send on"
    )
    if (endpoint.isBlank()) {
        throw IllegalArgumentException(
            "the endpoint is empty; a Loams client needs a base URL such as https://api.loams.dev, or a " +
                "Transport of its own"
        )
    }
    return HttpTransport(endpoint)
}