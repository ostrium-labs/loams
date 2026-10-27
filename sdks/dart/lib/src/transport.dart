import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import 'package:http/http.dart' as http;
import 'errors.dart';
import 'token_source.dart';

class HttpResponse {
  final int status;
  final Map<String, String> headers;
  final Uint8List bodyBytes;

  HttpResponse({required this.status, required this.headers, required this.bodyBytes});

  String get bodyText => utf8.decode(bodyBytes, allowMalformed: true);
}

class Transport {
  /// The client default for a unary call (R12).
  static const defaultTimeout = Duration(seconds: 30);

  final String endpoint;
  final TokenSource? tokenSource;
  final int maxRetries;

  /// A unary call's budget, retries included; null is none (R12).
  final Duration? timeout;
  final http.Client _client;

  /// [httpClient] replaces the default `http.Client` (a `MockClient` in tests).
  Transport(this.endpoint,
      {this.tokenSource, this.maxRetries = 3, this.timeout = defaultTimeout, http.Client? httpClient})
      : _client = httpClient ?? http.Client();

  /// The budget left for the attempt `callWithRetry` has in flight; a direct
  /// call (a stream) has none.
  Duration? _attemptTimeout;

  Future<HttpResponse> execute(String path, Map<String, String> headers, dynamic body) async {
    final timeout = _attemptTimeout;
    final cleanEndpoint = endpoint.endsWith('/') ? endpoint.substring(0, endpoint.length - 1) : endpoint;
    final cleanPath = path.startsWith('/') ? path.substring(1) : path;
    final uri = Uri.parse('$cleanEndpoint/$cleanPath');

    final effectiveHeaders = Map<String, String>.from(headers);
    if (tokenSource != null) {
      final token = await tokenSource!.getToken();
      if (token.isNotEmpty) {
        effectiveHeaders['authorization'] = 'Bearer $token';
      }
    }

    final request = http.Request('POST', uri);
    request.headers.addAll(effectiveHeaders);

    if (body is Uint8List) {
      request.bodyBytes = body;
    } else if (body is List<int>) {
      request.bodyBytes = Uint8List.fromList(body);
    } else if (body is String) {
      request.body = body;
    } else if (body is Map || body is List) {
      request.body = jsonEncode(body);
    }

    try {
      Future<HttpResponse> send() async {
        final streamedResponse = await _client.send(request);
        final respBytes = await streamedResponse.stream.toBytes();
        return HttpResponse(
          status: streamedResponse.statusCode,
          headers: streamedResponse.headers,
          bodyBytes: respBytes,
        );
      }

      return timeout == null ? await send() : await send().timeout(timeout);
    } on TimeoutException {
      throw DeadlineExceededException.client();
    } catch (e) {
      if (e is LoamsException) rethrow;
      throw LoamsException('Transport error: $e', code: 'unavailable', httpStatus: 503);
    }
  }

  Future<HttpResponse> callWithRetry(
    String path,
    Map<String, String> headers,
    dynamic body, {
    bool isMutation = false,
    String? idempotencyKey,
    Duration? timeout,
    bool useDefaultTimeout = true,
  }) async {
    int attempt = 0;
    bool refreshed = false;
    // R12: one budget for the call, retries included.
    final budget = timeout ?? (useDefaultTimeout ? this.timeout : null);
    final deadline = budget == null ? null : DateTime.now().add(budget);

    while (true) {
      attempt++;
      final remaining = deadline?.difference(DateTime.now());
      if (remaining != null && remaining <= Duration.zero) {
        throw DeadlineExceededException.client();
      }
      final currentHeaders = Map<String, String>.from(headers);
      if (remaining != null) {
        currentHeaders['connect-timeout-ms'] = '${remaining.inMilliseconds < 1 ? 1 : remaining.inMilliseconds}';
      }
      if (tokenSource != null) {
        final token = await tokenSource!.getToken();
        if (token.isNotEmpty) {
          currentHeaders['authorization'] = 'Bearer $token';
        }
      }
      if (idempotencyKey != null) {
        currentHeaders['idempotency-key'] = idempotencyKey;
      }

      _attemptTimeout = remaining;
      final HttpResponse res;
      try {
        res = await execute(path, currentHeaders, body);
      } finally {
        _attemptTimeout = null;
      }

      if (res.status >= 200 && res.status < 300) {
        return res;
      }

      final err = ErrorParser.parse(res.status, res.bodyBytes);

      // R1: Token expired refresh once
      if (err.reason == 'token_expired' && !refreshed && tokenSource != null) {
        await tokenSource!.refresh();
        refreshed = true;
        continue;
      }

      // R2: Retriable status
      final isRetriable = (res.status == 503 || err.code == 'unavailable');
      if (isRetriable && (idempotencyKey != null || !isMutation) && attempt < maxRetries) {
        await Future.delayed(Duration(milliseconds: 20 * attempt));
        continue;
      }

      throw err;
    }
  }

  void close() {
    _client.close();
  }
}
