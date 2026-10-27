import '../consistency.dart';
import '../errors.dart';
import 'filter.dart';
import 'json.dart';

/// How a hybrid search fuses its retrievers' rankings.
class Fusion {
  Fusion._();

  /// Reciprocal rank fusion; no `k` leaves the server's default (60).
  static Map<String, dynamic> rrf([int? k]) => {
        'rrf': k == null ? <String, dynamic>{} : {'k': k}
      };

  /// Distribution-based score fusion.
  static Map<String, dynamic> dbsf() => {'dbsf': <String, dynamic>{}};

  /// A weighted sum, one weight per retriever.
  static Map<String, dynamic> weighted(List<double> weights) => {
        'weightedSum': {'weights': weights}
      };
}

/// One sort key; `order` is `asc` or `desc`.
class Sort {
  Sort._();

  static Map<String, dynamic> score([String? order]) => {'score': _order(order)};

  /// The primary key: the tiebreaker `queryAll` requires.
  static Map<String, dynamic> pk([String? order]) => {'pk': _order(order)};

  /// `missing` is `first` or `last`.
  static Map<String, dynamic> field(String name, {String? order, String? missing}) => {
        'field': compact({
          'field': name,
          'order': order == null ? null : _orderName(order),
          'missing': missing == null ? null : 'MISSING_ORDER_${missing.toUpperCase()}',
        })
      };

  static Map<String, dynamic> _order(String? order) =>
      order == null ? <String, dynamic>{} : {'order': _orderName(order)};

  static String _orderName(String order) => order == 'asc' ? 'SORT_ORDER_ASC' : 'SORT_ORDER_DESC';
}

/// `strong`, `eventual`, or a token (checked here) as `atLeast`.
Map<String, dynamic>? consistencyJson(Object? level) {
  if (level == null) return null;
  if (level is Map<String, dynamic>) return level;
  if (level == 'strong') return {'freshness': 'FRESHNESS_STRONG'};
  if (level == 'eventual') return {'freshness': 'FRESHNESS_EVENTUAL'};
  if (level is ConsistencyToken) return {'atLeast': level.toString()};
  return {'atLeast': ConsistencyToken.parse(level.toString()).toString()};
}

/// A `SearchRequest`, verb by verb (SDKP §2.4):
///
/// ```dart
/// Search.on('notes')
///     .text('refund', fields: ['body'])
///     .vector('embedding', [1, 0, 0, 0])
///     .fuse(Fusion.rrf(60))
///     .filter(Filter.eq('tags', 'policy'))
///     .limit(2)
/// ```
///
/// [toJson] is the request exactly as built (what the builder corpus pins);
/// [build] checks it too, and `search.query` sends [wire] of it.
class Search implements JsonBuilder {
  /// The page size `queryAll` uses when the request sets no `limit`.
  static const defaultK = 10;

  final Map<String, dynamic> _req;

  Search._(this._req);

  factory Search.on(String collection, {String? namespace}) =>
      Search._(compact({'namespace': namespace, 'collection': collection}));

  /// A request from its proto3 JSON.
  factory Search.fromJson(Map<String, dynamic> json) => Search._(_copy(json));

  Search namespace(String namespace) => _with('namespace', namespace);
  Search collection(String collection) => _with('collection', collection);
  String? get namespaceName => _req['namespace'] as String?;

  /// A text retriever: a [Filter] (any query), or a string — a match on one
  /// field, a multi-match on several, a query string with none.
  Search text(Object query, {List<String>? fields, int? k}) {
    final q = query is Filter
        ? query.toJson()
        : (fields == null || fields.isEmpty)
            ? Filter.queryString(query.toString()).toJson()
            : fields.length == 1
                ? Filter.match(fields.first, query.toString()).toJson()
                : Filter.multiMatch(fields, query.toString()).toJson();
    return _retriever('text', compact({'query': q, 'k': k}));
  }

  /// A dense-vector (kNN) retriever; the ANN params are `AnnParams`.
  Search vector(String field, List<num> values,
      {int? k,
      bool? exact,
      int? nprobes,
      int? refineFactor,
      int? ef,
      double? oversampling,
      String? distance,
      Filter? filter}) {
    final params = compact({
      'exact': exact,
      'nprobes': nprobes,
      'refineFactor': refineFactor,
      'ef': ef,
      'oversampling': oversampling,
      'distance': distance,
    });
    return _retriever(
        'vector',
        compact({
          'field': field,
          'query': [for (final v in values) v.toDouble()],
          'k': k,
          'params': params.isEmpty ? null : params,
          'filter': filter?.toJson(),
        }));
  }

  /// A sparse-vector retriever over `indices` and their `values`.
  Search sparse(String field, List<int> indices, List<double> values, {int? k, Filter? filter}) => _retriever(
      'sparse',
      compact({
        'field': field,
        'query': {'indices': indices, 'values': values},
        'k': k,
        'filter': filter?.toJson(),
      }));

  /// `Fusion.rrf()`, `Fusion.dbsf()` or `Fusion.weighted(…)`.
  Search fuse(Map<String, dynamic> fusion) => _with('fusion', fusion);

  Search filter(Filter filter) => _with('filter', filter.toJson());

  /// `Sort.score()`, `Sort.pk()`, `Sort.field(…)`, or `'score'` / `'pk'`.
  Search sort(List<Object> keys) => _with('sort', [
        for (final key in keys)
          key == 'score'
              ? Sort.score()
              : key == 'pk'
                  ? Sort.pk()
                  : key
      ]);

  Search limit(int limit) => _with('limit', limit);
  Search offset(int offset) => _with('offset', offset);

  /// Continue after a hit: its `sortValues`.
  Search after(List<Object?> sortValues) => _with('searchAfter', List.of(sortValues));

  Search minScore(double score) => _with('scoreThreshold', score);

  /// The projection: which `_source` paths (or `noSource`), vectors and typed
  /// fields to return.
  Search select(
      {List<String>? include,
      List<String>? exclude,
      bool noSource = false,
      List<String>? vectors,
      List<String>? fields}) {
    final source = compact({'include': include, 'exclude': exclude});
    return _with(
        'select',
        compact({
          'source': noSource ? 'none' : (source.isEmpty ? null : source),
          'vectors': vectors,
          'fields': fields,
        }));
  }

  Search highlight(String field, {String? pre, String? post, int? fragmentSize, int? fragments}) {
    final existing = (_req['highlight'] as Map<String, dynamic>?)?['fields'] as List? ?? const [];
    return _with('highlight', {
      'fields': [
        ...existing,
        compact({
          'field': field,
          'preTag': pre,
          'postTag': post,
          'fragmentSize': fragmentSize,
          'numberOfFragments': fragments,
        })
      ]
    });
  }

  Search groupBy(String field, {int? size, int? limit}) =>
      _with('groupBy', compact({'field': field, 'groupSize': size, 'limit': limit}));

  /// Graph expansion of the hits (`expand_spec`). `direction` is `out`, `in`
  /// or `both`; `output` is `nested` or `flat`.
  Search expand(String graph,
          {String? fromField,
          String? seedLabel,
          int? seeds,
          int? hops,
          String? direction,
          List<String>? edgeTypes,
          int? limitPerSeed,
          int? limit,
          String? output}) =>
      _with(
          'expandSpec',
          compact({
            'graph': graph,
            'fromField': fromField,
            'seedLabel': seedLabel,
            'seeds': seeds,
            'hops': hops,
            'limitPerSeed': limitPerSeed,
            'limit': limit,
            'direction': direction == null ? null : 'EXPAND_DIRECTION_${direction.toUpperCase()}',
            'edgeTypes': edgeTypes,
            'output': output == null ? null : 'EXPAND_OUTPUT_${output.toUpperCase()}',
          }));

  /// `strong`, `eventual`, or a token as `atLeast`.
  Search consistency(Object level) => _with('consistency', consistencyJson(level));

  /// Whether the last sort key is the primary key, which `queryAll` needs.
  bool get hasTiebreak => tiebreakOf(_req);

  static bool tiebreakOf(Map<String, dynamic> request) {
    final sort = request['sort'] as List?;
    return sort != null && sort.isNotEmpty && sort.last is Map && (sort.last as Map).containsKey('pk');
  }

  @override
  Map<String, dynamic> toJson() => _copy(_req);

  /// [toJson], checked: a weighted fusion needs one weight per retriever.
  Map<String, dynamic> build() => check(toJson());

  static Map<String, dynamic> check(Map<String, dynamic> request) {
    final weights = ((request['fusion'] as Map?)?['weightedSum'] as Map?)?['weights'] as List?;
    final count = (request['retrievers'] as List?)?.length ?? 0;
    if (weights != null && weights.length != count) {
      throw InvalidArgumentException(
        'a weighted fusion needs one weight per retriever: ${weights.length} weights for $count retrievers',
        code: 'invalid_argument',
        httpStatus: 400,
        rawDetails: const [
          {
            'type': 'loams.errors.v1.ErrorInfo',
            'metadata': {'field': 'fusion.weighted_sum.weights'}
          }
        ],
      );
    }
    return request;
  }

  /// What the app modules send: [build]. A retriever's `k` goes out only when
  /// the caller gave one; the engine fills `max(10, offset + limit)` itself
  /// (SDKP-15, SDKP-18).
  static Map<String, dynamic> wire(Object request) =>
      check(request is Search ? request.toJson() : _copy(request as Map<String, dynamic>));

  Search _with(String key, Object? value) {
    final next = _copy(_req);
    if (value == null) {
      next.remove(key);
    } else {
      next[key] = value;
    }
    return Search._(next);
  }

  Search _retriever(String kind, Map<String, dynamic> body) => _with('retrievers', [
        ...(_req['retrievers'] as List? ?? const []),
        {kind: body}
      ]);

  static Map<String, dynamic> _copy(Map<String, dynamic> json) => _deep(json) as Map<String, dynamic>;

  static Object? _deep(Object? value) {
    if (value is Map) {
      return <String, dynamic>{for (final e in value.entries) e.key.toString(): _deep(e.value)};
    }
    if (value is List) {
      return [for (final v in value) _deep(v)];
    }
    return value;
  }
}
