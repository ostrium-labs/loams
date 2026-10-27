import 'dart:async';

/// R14: `streams.subscribe`, a loop over unary `Fetch` (SDKP-2), as a
/// `Stream`.
///
/// The next offset is the last record's offset + 1. An empty fetch backs off
/// 100 ms, doubling up to 1 s, and a non-empty one resets it. [offset] is where
/// the next fetch starts, so a caller can resume from it; cancelling the
/// stream's subscription (or [cancel]) stops the loop.
class Subscription<R> {
  static const initialBackoff = Duration(milliseconds: 100);
  static const maxBackoff = Duration(seconds: 1);

  final Future<List<R>> Function(int offset) _fetch;
  final int Function(R record) _offsetOf;
  final Future<void> Function(Duration) _sleep;
  int _offset;
  bool _cancelled = false;

  Subscription(this._fetch, this._offsetOf, {int fromOffset = 0, Future<void> Function(Duration)? sleep})
      : _offset = fromOffset,
        _sleep = sleep ?? ((d) => Future.delayed(d));

  /// Where the next fetch starts.
  int get offset => _offset;

  bool get cancelled => _cancelled;

  void cancel() => _cancelled = true;

  /// The records, as they arrive.
  Stream<R> get records async* {
    var backoff = initialBackoff;
    while (!_cancelled) {
      final records = await _fetch(_offset);
      if (records.isEmpty) {
        await _sleep(backoff);
        final doubled = backoff * 2;
        backoff = doubled > maxBackoff ? maxBackoff : doubled;
        continue;
      }
      backoff = initialBackoff;
      for (final record in records) {
        if (_cancelled) return;
        _offset = _offsetOf(record) + 1;
        yield record;
      }
    }
  }
}
