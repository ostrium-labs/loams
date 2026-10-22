import 'errors.dart';

/// One `s<stream>/p<partition>@<offset>` of a [ConsistencyToken].
class TokenItem {
  final BigInt stream;
  final int partition;
  final BigInt offset;

  const TokenItem(this.stream, this.partition, this.offset);

  @override
  bool operator ==(Object other) =>
      other is TokenItem && other.stream == stream && other.partition == partition && other.offset == offset;

  @override
  int get hashCode => Object.hash(stream, partition, offset);

  @override
  String toString() => 's$stream/p$partition@$offset';
}

/// A consistency token (runtime contract R4): `v1:s<stream>/p<partition>@<offset>,…`,
/// the encoding `crates/loams-collection/src/token.rs` defines.
///
/// A write answers with one and a read accepts one as `atLeast`, so a caller
/// that just wrote can read its own write. Two tokens merge by keeping the
/// largest offset per `(stream, partition)`. Streams and offsets are u64, so
/// they are [BigInt]s.
class ConsistencyToken {
  static const _prefix = 'v1:';
  static final _u64Max = BigInt.parse('18446744073709551615');
  static final _u32Max = BigInt.from(0xFFFFFFFF);
  static final _item = RegExp(r'^s(0|[1-9][0-9]*)/p(0|[1-9][0-9]*)@(0|[1-9][0-9]*)$');

  /// The items, in the order the token spells them.
  final List<TokenItem> items;

  ConsistencyToken._(this.items);

  /// Parses [text], keeping its order. Throws [InvalidArgumentException]
  /// (client-side, no network call) when it is not a `v1:` token.
  factory ConsistencyToken.parse(String text) {
    if (!text.startsWith(_prefix)) {
      throw _invalid(text);
    }
    final body = text.substring(_prefix.length);
    if (body.isEmpty) {
      return ConsistencyToken._(const []);
    }
    final items = <TokenItem>[];
    for (final part in body.split(',')) {
      final m = _item.firstMatch(part);
      if (m == null) {
        throw _invalid(text);
      }
      final stream = BigInt.parse(m.group(1)!);
      final partition = BigInt.parse(m.group(2)!);
      final offset = BigInt.parse(m.group(3)!);
      if (stream > _u64Max || partition > _u32Max || offset > _u64Max) {
        throw _invalid(text);
      }
      items.add(TokenItem(stream, partition.toInt(), offset));
    }
    return ConsistencyToken._(List.unmodifiable(items));
  }

  /// Whether [text] parses.
  static bool isValid(String text) {
    try {
      ConsistencyToken.parse(text);
      return true;
    } on InvalidArgumentException {
      return false;
    }
  }

  bool get isEmpty => items.isEmpty;

  /// Sorted by stream then partition, one item per pair (the largest offset).
  ConsistencyToken normalized() {
    final sorted = [...items]..sort((a, b) {
        final s = a.stream.compareTo(b.stream);
        if (s != 0) return s;
        final p = a.partition.compareTo(b.partition);
        return p != 0 ? p : a.offset.compareTo(b.offset);
      });
    final out = <TokenItem>[];
    for (final item in sorted) {
      if (out.isNotEmpty && out.last.stream == item.stream && out.last.partition == item.partition) {
        out[out.length - 1] = item;
      } else {
        out.add(item);
      }
    }
    return ConsistencyToken._(List.unmodifiable(out));
  }

  /// A new token holding the larger offset of the two for every
  /// `(stream, partition)`, normalised.
  ConsistencyToken merge(Object other) {
    final o = other is ConsistencyToken ? other : ConsistencyToken.parse(other.toString());
    return ConsistencyToken._([...items, ...o.items]).normalized();
  }

  /// The offset this token holds for one partition, or null.
  BigInt? offset(BigInt stream, int partition) {
    BigInt? best;
    for (final item in items) {
      if (item.stream == stream && item.partition == partition && (best == null || item.offset > best)) {
        best = item.offset;
      }
    }
    return best;
  }

  @override
  String toString() => '$_prefix${items.join(',')}';

  @override
  bool operator ==(Object other) => other is ConsistencyToken && other.toString() == toString();

  @override
  int get hashCode => toString().hashCode;

  static InvalidArgumentException _invalid(String text) => InvalidArgumentException(
        'invalid consistency token "$text"',
        code: 'invalid_argument',
        reason: 'invalid_consistency_token',
        httpStatus: 400,
      );
}

/// The session store (R4, `sessionConsistency: true`): every token an answer
/// carries is merged in, and every read that leaves `consistency` unset is
/// sent with `atLeast` set to the session's token.
///
/// One token per proto package: a graph's tokens name the graph's own stream
/// and a graph read refuses a token of another stream.
class SessionStore {
  /// The response fields a token is read from, in proto3 JSON.
  static const tokenFields = ['token', 'readToken', 'consistencyToken'];

  final Map<String, ConsistencyToken> _tokens = {};

  ConsistencyToken? token([String scope = 'loams.collection.v1']) => _tokens[scope];

  /// Merges [text] in; a token that does not parse is ignored.
  void observe(String scope, String? text) {
    if (text == null || text.isEmpty || !ConsistencyToken.isValid(text)) {
      return;
    }
    final parsed = ConsistencyToken.parse(text);
    final current = _tokens[scope];
    _tokens[scope] = current == null ? parsed.normalized() : current.merge(parsed);
  }

  void observeAnswer(String scope, Map<String, dynamic> answer) {
    for (final field in tokenFields) {
      final value = answer[field];
      if (value is String) {
        observe(scope, value);
      }
    }
  }

  /// Sets `consistency.atLeast` when the request leaves `consistency` unset.
  Map<String, dynamic> apply(String scope, Map<String, dynamic> request) {
    if (request['consistency'] != null) {
      return request;
    }
    final current = _tokens[scope];
    if (current == null || current.isEmpty) {
      return request;
    }
    return {
      ...request,
      'consistency': {'atLeast': current.toString()}
    };
  }
}
