import 'dart:convert';
import 'dart:io';

import 'package:loams/loams.dart';

/// Drives the builders from the builder corpus's neutral notation
/// (`sdks/fixtures/builders/README.md`): `{"op": verb, "args": [...]}`, where
/// an argument with an `op` is itself a call and a trailing plain object is the
/// call's options (`snake_case` names mapped to Dart's `camelCase`).
class BuilderCorpus {
  static String get dir {
    final env = Platform.environment['LOAMS_FIXTURES_DIR'];
    return env != null && env.isNotEmpty ? env : '../fixtures';
  }

  static List<Map<String, dynamic>> cases() {
    final builders = Directory('$dir/builders');
    if (!builders.existsSync()) return const [];
    final files = builders.listSync().whereType<File>().where((f) => f.path.endsWith('.json')).toList()
      ..sort((a, b) => a.path.compareTo(b.path));
    return [for (final f in files) jsonDecode(f.readAsStringSync()) as Map<String, dynamic>];
  }

  static Map<String, dynamic> build(Map<String, dynamic> c) {
    switch (c['builder']) {
      case 'schema':
        var schema = Schema();
        for (final call in c['spec'] as List) {
          schema = _schema(schema, call as Map<String, dynamic>);
        }
        return schema.toJson();
      case 'filter':
        return _filter(c['spec'] as Map<String, dynamic>).toJson();
      case 'search':
        final spec = c['spec'] as Map<String, dynamic>;
        var search = Search.on(spec['on'] as String);
        for (final call in spec['calls'] as List) {
          search = _search(search, call as Map<String, dynamic>);
        }
        return search.build();
    }
    throw StateError('unknown builder ${c['builder']}');
  }

  static (List<dynamic>, Map<String, dynamic>) _args(Map<String, dynamic> call, {bool options = true}) {
    final args = List<dynamic>.from(call['args'] as List? ?? const []);
    var named = <String, dynamic>{};
    if (options && args.isNotEmpty && args.last is Map && !(args.last as Map).containsKey('op')) {
      named = {for (final e in (args.removeLast() as Map).entries) _camel(e.key as String): e.value};
    }
    return (args, named);
  }

  static String _camel(String snake) {
    final parts = snake.split('_');
    return parts.first + parts.skip(1).map((p) => p[0].toUpperCase() + p.substring(1)).join();
  }

  static Schema _schema(Schema s, Map<String, dynamic> call) {
    final (args, o) = _args(call);
    final name = args.first as String;
    switch (call['op']) {
      case 'text':
        return s.text(name);
      case 'keyword':
        return s.keyword(name);
      case 'i64':
        return s.i64(name);
      case 'f64':
        return s.f64(name);
      case 'bool':
        return s.boolean(name);
      case 'timestamp':
        return s.timestamp(name);
      case 'uuid':
        return s.uuid(name);
      case 'vector':
        return s.vector(name, args[1] as int,
            distance: (o['distance'] ?? (args.length > 2 ? args[2] : 'cosine')) as String);
      case 'sparse_vector':
        return s.sparseVector(name);
      case 'dynamic':
        return s.dynamicMapping(name);
      case 'max_fields':
        return s.maxFields(args.first as int);
    }
    throw StateError('unknown schema op ${call['op']}');
  }

  static Filter _filter(Map<String, dynamic> call) {
    final (args, o) = _args(call, options: call['op'] != 'ids');
    List<Filter> filters() => [for (final a in args) _filter(a as Map<String, dynamic>)];
    switch (call['op']) {
      case 'all':
        return Filter.all();
      case 'none':
        return Filter.none();
      case 'eq':
        return Filter.eq(args[0] as String, args[1]);
      case 'in':
        return Filter.isIn(args[0] as String, List<Object?>.from(args[1] as List));
      case 'range':
        return Filter.range(args[0] as String, gt: o['gt'], gte: o['gte'], lt: o['lt'], lte: o['lte']);
      case 'exists':
        return Filter.exists(args[0] as String);
      case 'is_null':
        return Filter.isNull(args[0] as String);
      case 'is_empty':
        return Filter.isEmpty(args[0] as String);
      case 'prefix':
        return Filter.prefix(args[0] as String, args[1] as String);
      case 'wildcard':
        return Filter.wildcard(args[0] as String, args[1] as String);
      case 'match':
        return Filter.match(args[0] as String, args[1] as String);
      case 'phrase':
        return Filter.phrase(args[0] as String, args[1] as String);
      case 'multi_match':
        return Filter.multiMatch(List<String>.from(args[0] as List), args[1] as String);
      case 'query_string':
        return Filter.queryString(args[0] as String);
      case 'ids':
        return Filter.ids([for (final a in args) a as Object]);
      case 'and':
        return Filter.and(filters());
      case 'or':
        return Filter.or(filters());
      case 'not':
        return Filter.not(filters());
      case 'filter':
        return Filter.filter(filters());
    }
    throw StateError('unknown filter op ${call['op']}');
  }

  static Object _nested(Map<String, dynamic> call) {
    final (args, _) = _args(call);
    switch (call['op']) {
      case 'rrf':
        return Fusion.rrf(args.isEmpty ? null : args.first as int);
      case 'dbsf':
        return Fusion.dbsf();
      case 'weighted':
        return Fusion.weighted([for (final w in args) (w as num).toDouble()]);
      case 'score':
        return Sort.score(args.isEmpty ? null : args.first as String);
      case 'pk':
        return Sort.pk(args.isEmpty ? null : args.first as String);
      case 'field':
        return Sort.field(args.first as String);
    }
    return _filter(call);
  }

  static Search _search(Search s, Map<String, dynamic> call) {
    final (args, o) = _args(call);
    switch (call['op']) {
      case 'text':
        final q = args[0];
        return s.text(q is Map ? _filter(q as Map<String, dynamic>) : q as Object,
            fields: o['fields'] == null ? null : List<String>.from(o['fields'] as List), k: o['k'] as int?);
      case 'vector':
        return s.vector(args[0] as String, List<num>.from(args[1] as List),
            k: o['k'] as int?,
            exact: o['exact'] as bool?,
            nprobes: o['nprobes'] as int?,
            refineFactor: o['refineFactor'] as int?,
            ef: o['ef'] as int?,
            distance: o['distance'] as String?);
      case 'fuse':
        return s.fuse(_nested(args[0] as Map<String, dynamic>) as Map<String, dynamic>);
      case 'filter':
        return s.filter(_filter(args[0] as Map<String, dynamic>));
      case 'sort':
        return s.sort([for (final k in args) _nested(k as Map<String, dynamic>)]);
      case 'limit':
        return s.limit(args[0] as int);
      case 'offset':
        return s.offset(args[0] as int);
      case 'after':
        return s.after(List<Object?>.from(args[0] as List));
      case 'min_score':
        return s.minScore((args[0] as num).toDouble());
      case 'select':
        return s.select(
          include: o['include'] == null ? null : List<String>.from(o['include'] as List),
          exclude: o['exclude'] == null ? null : List<String>.from(o['exclude'] as List),
          noSource: o['noSource'] == true,
        );
      case 'highlight':
        return s.highlight(args[0] as String);
      case 'group_by':
        return s.groupBy(args[0] as String);
      case 'expand':
        return s.expand(args[0] as String,
            fromField: o['fromField'] as String?, hops: o['hops'] as int?, direction: o['direction'] as String?);
      case 'consistency':
        return s.consistency(args[0] as Object);
    }
    throw StateError('unknown search op ${call['op']}');
  }

  /// Proto3 JSON compared as values: numbers by value, key order ignored.
  static Object? normalize(Object? v) {
    if (v is Map) {
      final keys = v.keys.map((k) => k.toString()).toList()..sort();
      return {for (final k in keys) k: normalize(v[k])};
    }
    if (v is List) return [for (final x in v) normalize(x)];
    if (v is num) return v.toDouble();
    return v;
  }
}
