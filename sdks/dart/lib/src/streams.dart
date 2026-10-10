import 'dart:convert';
import 'dart:typed_data';
import 'envelopes.dart';
import 'errors.dart';

class StreamHandle {
  final List<String> frameKinds = [];
  String? lastCursor;
  bool _closed = false;
  final List<Frame> frames;

  StreamHandle(this.frames, {this.lastCursor});

  Stream<dynamic> get messages async* {
    for (final frame in frames) {
      if (_closed) return;

      if (frame.isTrailer) {
        frameKinds.add('trailer');
        try {
          final text = utf8.decode(frame.payload, allowMalformed: true);
          final decoded = jsonDecode(text);
          if (decoded is Map && decoded['error'] != null) {
            throw ErrorParser.parse(400, decoded['error']);
          }
        } catch (e) {
          if (e is LoamsException) rethrow;
        }
        continue;
      }

      frameKinds.add('message');
      try {
        final text = utf8.decode(frame.payload, allowMalformed: true);
        final msg = jsonDecode(text);
        if (msg is Map) {
          if (msg['cursor'] is String) {
            lastCursor = msg['cursor'] as String;
          }
          if (msg['heartbeat'] != null) {
            frameKinds.add('heartbeat');
            continue;
          }
          yield msg;
        } else {
          yield text;
        }
      } catch (_) {
        yield frame.payload;
      }
    }
  }

  void close() {
    _closed = true;
  }
}
