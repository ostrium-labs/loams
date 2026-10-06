package dev.loams.transport

import com.connectrpc.ConnectException
import com.connectrpc.Interceptor
import com.connectrpc.ProtocolClientConfig
import com.connectrpc.StreamFunction
import com.connectrpc.StreamType
import com.connectrpc.UnaryFunction
import com.connectrpc.extensions.GoogleJavaLiteProtobufStrategy
import com.connectrpc.http.clone
import com.connectrpc.impl.ProtocolClient
import com.connectrpc.okhttp.ConnectOkHttpClient
import com.connectrpc.protocols.GETConfiguration
import com.connectrpc.protocols.NetworkProtocol
import dev.loams.core.errors.Reason
import dev.loams.proto.loams.approvals.v1.ApprovalServiceClient
import dev.loams.proto.loams.devices.v1.DeviceServiceClient
import dev.loams.proto.loams.errors.v1.ErrorInfo
import dev.loams.proto.loams.instance.v1.InstanceServiceClient
import dev.loams.proto.loams.notifications.v1.NotificationServiceClient
import dev.loams.proto.loams.operations.v1.OperationsServiceClient
import java.util.Locale
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.Dispatchers
import okhttp3.OkHttpClient

/** Supplies the current access token, or null before sign-in. */
fun interface TokenSource {
    fun accessToken(): String?
}

/**
 * The generated Connect clients for one instance: Connect protocol, binary protobuf (javalite),
 * reads sent as GET where marked NO_SIDE_EFFECTS, 10 s deadlines for unary calls and none for
 * streams (design §37 §8.3).
 */
class Clients(
    val baseUrl: String,
    http: OkHttpClient,
    tokens: TokenSource,
    locale: () -> Locale = Locale::getDefault,
) {
    private val protocol = ProtocolClient(
        httpClient = ConnectOkHttpClient(http),
        config = ProtocolClientConfig(
            host = baseUrl.trimEnd('/'),
            serializationStrategy = GoogleJavaLiteProtobufStrategy(),
            networkProtocol = NetworkProtocol.CONNECT,
            getConfiguration = GETConfiguration.EnabledWithFallback(),
            interceptors = listOf({ HeadersInterceptor(tokens, locale) }),
            ioCoroutineContext = Dispatchers.IO,
            timeoutOracle = { spec -> if (spec.streamType == StreamType.UNARY) 10.seconds else null },
        ),
    )

    val instance = InstanceServiceClient(protocol)
    val devices = DeviceServiceClient(protocol)
    val approvals = ApprovalServiceClient(protocol)
    val operations = OperationsServiceClient(protocol)
    val notifications = NotificationServiceClient(protocol)
}

/** Adds the bearer token and Accept-Language (server-rendered text, AP0 Ruling 8). */
private class HeadersInterceptor(private val tokens: TokenSource, private val locale: () -> Locale) : Interceptor {
    private fun headers(current: Map<String, List<String>>): Map<String, List<String>> {
        val out = current.toMutableMap()
        // TODO(auth plan, Q438): "DPoP <token>" plus a DPoP proof header signed by dpop-<instance>.
        tokens.accessToken()?.let { out["authorization"] = listOf("Bearer $it") }
        out["accept-language"] = listOf(locale().toLanguageTag())
        return out
    }

    override fun unaryFunction() = UnaryFunction(requestFunction = { it.clone(headers = headers(it.headers)) })

    override fun streamFunction() = StreamFunction(requestFunction = { it.clone(headers = headers(it.headers)) })
}

/** The stable reason of a failed call (AP0 Ruling 6). */
fun ConnectException.reason(): Reason = Reason.fromWire(unpackedDetails(ErrorInfo::class).firstOrNull()?.reason)
