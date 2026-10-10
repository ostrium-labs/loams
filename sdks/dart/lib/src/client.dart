import 'dart:convert';
import 'dart:typed_data';
import 'envelopes.dart';
import 'errors.dart';
import 'idempotency.dart';
import 'streams.dart';
import 'token_source.dart';
import 'transport.dart';

class InstanceService {
  final Transport _transport;
  InstanceService(this._transport);

  Future<Map<String, dynamic>> getInstance([Map<String, dynamic> request = const {}]) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.instance.v1.InstanceService/GetInstance', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }

  Future<Map<String, dynamic>> whoAmI() async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.instance.v1.InstanceService/WhoAmI', headers, '{}');
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }
}

class LiveService {
  final Transport _transport;
  LiveService(this._transport);

  Future<Map<String, dynamic>> query(Map<String, dynamic> request) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.live.v1.LiveService/Query', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }

  Future<StreamHandle> watch() async {
    final headers = {
      'content-type': 'application/connect+json',
      'connect-protocol-version': '1',
    };
    final body = Envelopes.pack(Uint8List.fromList(utf8.encode('{}')));
    final res = await _transport.execute('/loams.live.v1.LiveService/Watch', headers, body);
    if (res.status != 200) {
      throw ErrorParser.parse(res.status, res.bodyBytes);
    }
    final frames = Envelopes.split(res.bodyBytes);
    return StreamHandle(frames);
  }
}

class ApprovalService {
  final Transport _transport;
  ApprovalService(this._transport);

  Future<Map<String, dynamic>> decideApproval(Map<String, dynamic> request, {String? idempotencyKey}) async {
    final key = idempotencyKey ?? request['idempotencyKey'] ?? request['idempotency_key'] ?? Idempotency.mintKey();
    final payload = Map<String, dynamic>.from(request);
    payload['idempotencyKey'] = key;

    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry(
      '/loams.approvals.v1.ApprovalService/DecideApproval',
      headers,
      payload,
      isMutation: true,
      idempotencyKey: key,
    );
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }

  Future<StreamHandle> watchApprovals({String? cursor}) async {
    final headers = {
      'content-type': 'application/connect+json',
      'connect-protocol-version': '1',
    };
    final reqObj = cursor != null ? {'cursor': cursor} : <String, dynamic>{};
    final body = Envelopes.pack(Uint8List.fromList(utf8.encode(jsonEncode(reqObj))));
    final res = await _transport.execute('/loams.approvals.v1.ApprovalService/WatchApprovals', headers, body);
    if (res.status != 200) {
      throw ErrorParser.parse(res.status, res.bodyBytes);
    }
    final frames = Envelopes.split(res.bodyBytes);
    return StreamHandle(frames, lastCursor: cursor);
  }

  Future<Map<String, dynamic>> listApprovals([Map<String, dynamic> request = const {}]) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.approvals.v1.ApprovalService/ListApprovals', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }
}

class DeviceService {
  final Transport _transport;
  DeviceService(this._transport);

  Future<Map<String, dynamic>> sendTestNotification([Map<String, dynamic> request = const {}]) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.devices.v1.DeviceService/SendTestNotification', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }
}

class LoamsClient {
  final Transport transport;
  late final InstanceService instances;
  late final LiveService live;
  late final ApprovalService approvals;
  late final DeviceService devices;

  LoamsClient(
    String endpoint, {
    TokenSource? tokenSource,
    int maxRetries = 3,
  }) : transport = Transport(endpoint, tokenSource: tokenSource, maxRetries: maxRetries) {
    instances = InstanceService(transport);
    live = LiveService(transport);
    approvals = ApprovalService(transport);
    devices = DeviceService(transport);
  }

  void close() {
    transport.close();
  }
}
