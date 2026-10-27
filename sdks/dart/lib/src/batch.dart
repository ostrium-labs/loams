import 'dart:convert';

import 'consistency.dart';
import 'errors.dart';
import 'idempotency.dart';

/// What a `writeBatch` applied: the merged consistency token (null when no
/// chunk answered one), the chunks and the rows.
class BatchResult {
  final String? token;
  final int chunks;
  final int rows;

  const BatchResult({this.token, required this.chunks, required this.rows});
}

/// A `writeBatch` stopped at a failed chunk (R13): the chunks before it were
/// applied, and [token] is their merged consistency token.
class BatchException extends LoamsException {
  final LoamsException cause;
  final int chunksApplied;
  final int rowsApplied;
  final String? token;
  final Map<String, String> metadata;

  BatchException(this.cause, {required this.chunksApplied, required this.rowsApplied, this.token})
      : metadata = {
          'chunks_applied': '$chunksApplied',
          'rows_applied': '$rowsApplied',
          if (cause.reason != null) 'cause_reason': cause.reason!,
        },
        super(
          'writeBatch stopped at chunk ${chunksApplied + 1}: $chunksApplied chunks ($rowsApplied operations) '
          'were applied before it; ${cause.message}',
          // SDKP-23: registered under `aborted`; the failed chunk's error is the cause.
          code: 'aborted',
          reason: 'batch_partially_applied',
          httpStatus: cause.httpStatus,
          rawDetails: cause.rawDetails,
        );
}

/// R13: a large write as a sequence of `WriteDocuments` chunks. One chunk is in
/// flight at a time; a chunk closes at `chunkRows` ops or when the next op would
/// take it past `chunkBytes` of JSON. Each chunk gets its own idempotency key,
/// which the transport reuses on that chunk's retries.
class Batch {
  static const defaultChunkRows = 10000;
  static const defaultChunkBytes = 4 * 1024 * 1024;

  static Future<BatchResult> run(
    Iterable<Map<String, dynamic>> ops,
    Future<Map<String, dynamic>> Function(List<Map<String, dynamic>> chunk, String idempotencyKey) write, {
    int chunkRows = defaultChunkRows,
    int chunkBytes = defaultChunkBytes,
  }) async {
    if (chunkRows <= 0 || chunkBytes <= 0) {
      throw ArgumentError('chunkRows and chunkBytes must be positive');
    }
    ConsistencyToken? token;
    var applied = 0;
    var rows = 0;
    var chunk = <Map<String, dynamic>>[];
    var size = 0;

    Future<void> flush() async {
      if (chunk.isEmpty) return;
      final Map<String, dynamic> answer;
      try {
        answer = await write(chunk, Idempotency.mintKey());
      } on LoamsException catch (e) {
        throw BatchException(e, chunksApplied: applied, rowsApplied: rows, token: token?.toString());
      }
      final text = answer['token'];
      if (text is String && text.isNotEmpty && ConsistencyToken.isValid(text)) {
        final parsed = ConsistencyToken.parse(text);
        token = token == null ? parsed.normalized() : token!.merge(parsed);
      }
      applied++;
      rows += chunk.length;
      chunk = [];
      size = 0;
    }

    for (final op in ops) {
      final bytes = utf8.encode(jsonEncode(op)).length;
      if (chunk.isNotEmpty && (chunk.length >= chunkRows || size + bytes > chunkBytes)) {
        await flush();
      }
      chunk.add(op);
      size += bytes;
    }
    await flush();
    return BatchResult(token: token?.toString(), chunks: applied, rows: rows);
  }
}
