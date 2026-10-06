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
  final String endpoint;
  final TokenSource? tokenSource;
  final int maxRetries;
  final http.Client _client = http.Client();

  Transport(this.endpoint, {this.tokenSource, this.maxRetries = 3});

  Future<HttpResponse> execute(String path, Map<String, String> headers, dynamic body) async {
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
      final streamedResponse = await _client.send(request);
      final respBytes = await streamedResponse.stream.toBytes();

      return HttpResponse(
        status: streamedResponse.statusCode,
        headers: streamedResponse.headers,
        bodyBytes: respBytes,
      );
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
  }) async {
    int attempt = 0;
    bool refreshed = false;

    while (true) {
      attempt++;
      final currentHeaders = Map<String, String>.from(headers);
      if (tokenSource != null) {
        final token = await tokenSource!.getToken();
        if (token.isNotEmpty) {
          currentHeaders['authorization'] = 'Bearer $token';
        }
      }
      if (idempotencyKey != null) {
        currentHeaders['idempotency-key'] = idempotencyKey;
      }

      final res = await execute(path, currentHeaders, body);

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
