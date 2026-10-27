import 'json.dart';

/// The `CreateCollectionRequest.schema` Struct (SDKP §2.6), in the shape the
/// server's schema parser reads (`kb_schema()` in `connect_collections.rs`).
///
/// ```dart
/// Schema().keyword('title').text('body').i64('n').vector('embedding', 4)
/// ```
class Schema implements JsonBuilder {
  final List<Map<String, dynamic>> _fields = [];
  final List<Map<String, dynamic>> _vectors = [];
  final List<Map<String, dynamic>> _sparse = [];
  String _dynamic = 'ignore';
  int _maxFields = 1000;

  /// A full-text field; `positions` is what phrase queries need.
  Schema text(String name,
          {String analyzer = 'standard', bool positions = true, bool? indexed, bool? fast, String? sourcePath}) =>
      _field(
          name,
          {
            'text': {'analyzer': analyzer, 'positions': positions}
          },
          indexed ?? true,
          fast ?? false,
          sourcePath);

  Schema keyword(String name, {bool? indexed, bool? fast, String? sourcePath}) =>
      _field(name, 'keyword', indexed ?? true, fast ?? true, sourcePath);

  Schema i64(String name, {bool? indexed, bool? fast, String? sourcePath}) =>
      _field(name, 'i64', indexed ?? true, fast ?? true, sourcePath);

  Schema f64(String name, {bool? indexed, bool? fast, String? sourcePath}) =>
      _field(name, 'f64', indexed ?? true, fast ?? true, sourcePath);

  Schema boolean(String name, {bool? indexed, bool? fast, String? sourcePath}) =>
      _field(name, 'bool', indexed ?? true, fast ?? true, sourcePath);

  /// A date-time field (the schema's `date` kind).
  Schema timestamp(String name, {bool? indexed, bool? fast, String? sourcePath}) =>
      _field(name, 'date', indexed ?? true, fast ?? true, sourcePath);

  Schema uuid(String name, {bool? indexed, bool? fast, String? sourcePath}) =>
      _field(name, 'uuid', indexed ?? true, fast ?? true, sourcePath);

  Schema vector(String name, int dim, {String distance = 'cosine'}) {
    if (dim <= 0) {
      throw ArgumentError.value(dim, 'dim', "a vector's dim must be positive");
    }
    _vectors.add({'name': name, 'dim': dim, 'distance': distance});
    return this;
  }

  Schema sparseVector(String name, {String? modifier}) {
    _sparse.add(compact({'name': name, 'modifier': modifier}));
    return this;
  }

  /// `ignore` (the builder's default), `strict` (the server's) or `map`.
  Schema dynamicMapping(String mode) {
    if (!const ['ignore', 'strict', 'map'].contains(mode)) {
      throw ArgumentError.value(mode, 'mode', 'dynamic is ignore, strict or map');
    }
    _dynamic = mode;
    return this;
  }

  Schema maxFields(int count) {
    _maxFields = count;
    return this;
  }

  @override
  Map<String, dynamic> toJson() => {
        'fields': [for (final f in _fields) Map.of(f)],
        'vectors': [for (final v in _vectors) Map.of(v)],
        'sparse_vectors': [for (final s in _sparse) Map.of(s)],
        'dynamic': _dynamic,
        'max_fields': _maxFields,
      };

  Schema _field(String name, Object kind, bool indexed, bool fast, String? sourcePath) {
    _fields.add({
      'name': name,
      'source_path': sourcePath ?? name,
      'kind': kind,
      'indexed': indexed,
      'fast': fast,
    });
    return this;
  }
}
