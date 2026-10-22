import 'errors.dart';

/// Conversions between Dart values and the proto3 JSON the app modules send
/// and answer.
class Values {
  Values._();

  /// A primary key as a `DocumentId`'s JSON: a non-negative int is `uint`, a
  /// String is `string`, and `{uuid: …}` (any map naming its arm) passes through.
  static Map<String, dynamic> documentId(Object id) {
    if (id is int) {
      if (id < 0) {
        throw InvalidArgumentException('a document id is a non-negative int, a String or {uuid: …}',
            code: 'invalid_argument', httpStatus: 400);
      }
      return {'uint': id.toString()};
    }
    if (id is BigInt) {
      return {'uint': id.toString()};
    }
    if (id is String) {
      return {'string': id};
    }
    if (id is Map) {
      return Map<String, dynamic>.from(id);
    }
    throw InvalidArgumentException('a document id is a non-negative int, a String or {uuid: …}',
        code: 'invalid_argument', httpStatus: 400);
  }

  /// A `DocumentId`'s JSON back as a Dart value: an int (a BigInt past 2^63),
  /// a String, or the `{uuid: …}` map.
  static Object fromDocumentId(Map<String, dynamic> id) {
    final uint = id['uint'];
    if (uint != null) {
      final big = BigInt.parse(uint.toString());
      return big.isValidInt ? big.toInt() : big;
    }
    if (id['string'] != null) {
      return id['string'] as String;
    }
    return id;
  }

  /// A Dart value as a `loams.graph.v1.Value`'s JSON, for GQL parameters.
  static Map<String, dynamic> graphValue(Object? value) {
    if (value == null) return {'null': 'NULL_VALUE'};
    if (value is bool) return {'boolean': value};
    if (value is int) return {'int64': value.toString()};
    if (value is BigInt) return {'uint64': value.toString()};
    if (value is double) return {'float64': value};
    if (value is String) return {'string': value};
    if (value is List) {
      return {
        'list': {
          'values': [for (final v in value) graphValue(v)]
        }
      };
    }
    if (value is Map) {
      return {
        'map': {
          'entries': {for (final e in value.entries) e.key.toString(): graphValue(e.value)}
        }
      };
    }
    throw InvalidArgumentException('no graph value for ${value.runtimeType}',
        code: 'invalid_argument', httpStatus: 400);
  }

  /// A `loams.graph.v1.Value`'s JSON as a Dart value. Nodes, relationships,
  /// paths and temporal values stay as their JSON.
  static Object? fromGraphValue(Map<String, dynamic> value) {
    if (value.isEmpty) return null;
    final kind = value.keys.first;
    final inner = value[kind];
    switch (kind) {
      case 'null':
        return null;
      case 'boolean':
      case 'float64':
      case 'string':
        return inner;
      case 'int64':
      case 'uint64':
        final big = BigInt.parse(inner.toString());
        return big.isValidInt ? big.toInt() : big;
      case 'list':
        return [
          for (final v in ((inner as Map)['values'] as List? ?? const []))
            fromGraphValue(Map<String, dynamic>.from(v as Map))
        ];
      case 'map':
        return {
          for (final e in (((inner as Map)['entries'] as Map?) ?? const {}).entries)
            e.key.toString(): fromGraphValue(Map<String, dynamic>.from(e.value as Map))
        };
    }
    return value;
  }
}
