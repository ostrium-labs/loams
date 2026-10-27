import 'json.dart';

/// A primary key as `Query.ids` spells it: an int is `{uint: "…"}`, a string
/// stays a string, and `{uuid: …}` passes through.
Object idValue(Object id) {
  if (id is int) {
    if (id < 0) {
      throw ArgumentError.value(id, 'id', 'a numeric document id is non-negative');
    }
    return {'uint': id.toString()};
  }
  if (id is BigInt) {
    return {'uint': id.toString()};
  }
  if (id is String) {
    return id;
  }
  if (id is Map) {
    return Map<String, dynamic>.from(id);
  }
  throw ArgumentError.value(id, 'id', 'a document id is an int, a String or {uuid: …}');
}

/// One `loams.collection.v1.Query` (SDKP §2.4): the filter of a search, the
/// query of a text retriever, and the typed `where` of the document reads.
///
/// ```dart
/// Filter.and([Filter.eq('tags', 'policy'), Filter.range('n', gte: 2)])
/// ```
class Filter implements JsonBuilder {
  final Map<String, dynamic> _json;

  const Filter._(this._json);

  /// A filter from its proto3 JSON.
  factory Filter.fromJson(Map<String, dynamic> json) => Filter._(json);

  @override
  Map<String, dynamic> toJson() => _json;

  /// `this & other`, `this | other` and `~this`: [and], [or] and [not].
  Filter operator &(Filter other) => Filter.and([this, other]);
  Filter operator |(Filter other) => Filter.or([this, other]);
  Filter operator ~() => Filter.not([this]);

  static Filter all() => const Filter._({'matchAll': <String, dynamic>{}});
  static Filter none() => const Filter._({'matchNone': <String, dynamic>{}});

  /// `field` equals `value` (a term query).
  static Filter eq(String field, Object? value) => Filter._({
        'term': {'field': field, 'value': value}
      });

  /// `field` is one of `values` (a terms query). `in` is a Dart keyword.
  static Filter isIn(String field, List<Object?> values) => Filter._({
        'terms': {'field': field, 'values': values}
      });

  static Filter range(String field, {Object? gt, Object? gte, Object? lt, Object? lte}) => Filter._({
        'range': compact({'field': field, 'gt': gt, 'gte': gte, 'lt': lt, 'lte': lte})
      });

  static Filter exists(String field) => Filter._({
        'exists': {'field': field}
      });

  static Filter isNull(String field) => Filter._({
        'isNull': {'field': field}
      });

  static Filter isEmpty(String field) => Filter._({
        'isEmpty': {'field': field}
      });

  static Filter prefix(String field, String value) => Filter._({
        'prefix': {'field': field, 'value': value}
      });

  static Filter wildcard(String field, String pattern) => Filter._({
        'wildcard': {'field': field, 'pattern': pattern}
      });

  /// `edits` is a number of edits or `'auto'`; null leaves the server's default.
  static Filter fuzzy(String field, String value, {Object? edits}) => Filter._({
        'fuzzy': compact({'field': field, 'value': value, 'fuzziness': _fuzziness(edits)})
      });

  /// Full-text match; `operator` is `or` (default) or `and`.
  static Filter match(String field, String text,
          {String? operator, Object? minimumShouldMatch, Object? fuzziness, String? analyzer}) =>
      Filter._({
        'match': compact({
          'field': field,
          'text': text,
          'operator': operator,
          'minimumShouldMatch': minimumShouldMatch?.toString(),
          'fuzziness': _fuzziness(fuzziness),
          'analyzer': analyzer,
        })
      });

  static Filter phrase(String field, String text, {int? slop}) => Filter._({
        'matchPhrase': compact({'field': field, 'text': text, 'slop': slop})
      });

  /// One text over several fields: names or `name^boost`. `kind` is
  /// `best_fields`, `most_fields`, `cross_fields`, `phrase` or `phrase_prefix`.
  static Filter multiMatch(List<String> fields, String text, {String? kind, String? operator, double? tieBreaker}) =>
      Filter._({
        'multiMatch': compact({
          'fields': [for (final f in fields) _boosted(f)],
          'text': text,
          'kind': kind == null ? null : 'MULTI_MATCH_KIND_${kind.toUpperCase()}',
          'operator': operator == null ? null : _boolOperator(operator),
          'tieBreaker': tieBreaker,
        })
      });

  static Filter queryString(String query, {List<String>? fields, String? operator}) => Filter._({
        'queryString': compact({
          'query': query,
          'defaultFields': fields == null || fields.isEmpty ? null : fields,
          'defaultOperator': operator == null ? null : _boolOperator(operator),
        })
      });

  /// Documents by primary key: ints, strings or `{uuid: …}`.
  static Filter ids(List<Object> ids) => Filter._({
        'ids': [for (final id in ids) idValue(id)]
      });

  /// Every clause matches (`bool.must`).
  static Filter and(List<Filter> clauses) => Filter._({
        'bool': {'must': _list(clauses)}
      });

  /// Any clause matches (`bool.should`).
  static Filter or(List<Filter> clauses) => Filter._({
        'bool': {'should': _list(clauses)}
      });

  /// At least `minimumShouldMatch` of `clauses` match.
  static Filter anyOf(List<Filter> clauses, Object minimumShouldMatch) => Filter._({
        'bool': {'should': _list(clauses), 'minimumShouldMatch': minimumShouldMatch.toString()}
      });

  /// No clause matches (`bool.mustNot`).
  static Filter not(List<Filter> clauses) => Filter._({
        'bool': {'mustNot': _list(clauses)}
      });

  /// Every clause matches, without scoring (`bool.filter`).
  static Filter filter(List<Filter> clauses) => Filter._({
        'bool': {'filter': _list(clauses)}
      });

  static Filter boost(Filter clause, double boost) => Filter._({
        'boost': {'query': clause.toJson(), 'boost': boost}
      });

  static Filter constantScore(Filter clause, double score) => Filter._({
        'constantScore': {'query': clause.toJson(), 'score': score}
      });

  static List<Map<String, dynamic>> _list(List<Filter> clauses) => [for (final c in clauses) c.toJson()];

  static Map<String, dynamic>? _fuzziness(Object? edits) {
    if (edits == null) return null;
    if (edits == 'auto') return {'auto': <String, dynamic>{}};
    return {'edits': edits is int ? edits : int.parse(edits.toString())};
  }

  static Map<String, dynamic> _boosted(String spec) {
    final at = spec.lastIndexOf('^');
    if (at > 0) {
      final boost = double.tryParse(spec.substring(at + 1));
      if (boost != null && boost.isFinite) {
        return {'field': spec.substring(0, at), 'boost': boost};
      }
    }
    return {'field': spec};
  }

  static String _boolOperator(String operator) => operator == 'and' ? 'BOOL_OPERATOR_AND' : 'BOOL_OPERATOR_OR';
}
