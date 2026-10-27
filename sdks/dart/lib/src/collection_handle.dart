import 'batch.dart';
import 'builders/filter.dart';
import 'builders/search.dart';
import 'modules/app.dart';

/// The modules a [CollectionHandle] delegates to.
abstract class AppModules {
  DocumentsModule get documents;
  CollectionsModule get collections;
  SearchModule get search;
  VectorModule get vector;
}

/// `client.collection('notes')` (SDKP §2.8): every document and search call
/// with the collection (and namespace) filled in. It holds no state of its own.
class CollectionHandle {
  final AppModules _client;
  final String name;
  final String? namespace;

  CollectionHandle(this._client, this.name, {this.namespace});

  Future<Map<String, dynamic>> upsert(Iterable<Doc> docs, {CallOptions options = const CallOptions()}) =>
      _client.documents.upsert(name, docs, namespace: namespace, options: options);

  Future<Map<String, dynamic>> get(List<Object> ids,
          {Map<String, dynamic>? select, Object? consistency, CallOptions options = const CallOptions()}) =>
      _client.documents
          .get(name, ids, select: select, consistency: consistency, namespace: namespace, options: options);

  Future<Map<String, dynamic>> delete(List<Object> ids, {CallOptions options = const CallOptions()}) =>
      _client.documents.delete(name, ids, namespace: namespace, options: options);

  Future<Map<String, dynamic>> patch(Object id,
          {Map<String, dynamic>? set, List<String>? unset, CallOptions options = const CallOptions()}) =>
      _client.documents.patch(name, id, set: set, unset: unset, namespace: namespace, options: options);

  Future<BatchResult> writeBatch(Iterable<Doc> docs,
          {int chunkRows = Batch.defaultChunkRows,
          int chunkBytes = Batch.defaultChunkBytes,
          CallOptions options = const CallOptions()}) =>
      _client.documents
          .writeBatch(name, docs, chunkRows: chunkRows, chunkBytes: chunkBytes, namespace: namespace, options: options);

  Future<Map<String, dynamic>> count(
          {Filter? where, Object? filter, Object? consistency, CallOptions options = const CallOptions()}) =>
      _client.documents
          .count(name, where: where, filter: filter, consistency: consistency, namespace: namespace, options: options);

  Stream<Map<String, dynamic>> scrollAll(
          {Filter? where, Object? filter, int? limit, CallOptions options = const CallOptions()}) =>
      _client.documents
          .scrollAll(name, where: where, filter: filter, limit: limit, namespace: namespace, options: options);

  /// [search]'s collection is set to this one; without one, a fresh
  /// `Search.on(name)`.
  Future<Map<String, dynamic>> search([Search? search, CallOptions options = const CallOptions()]) {
    var s = (search ?? Search.on(name)).collection(name);
    if (namespace != null && s.namespaceName == null) {
      s = s.namespace(namespace!);
    }
    return _client.search.query(s, options: options);
  }

  Future<Map<String, dynamic>> vectorSearch(List<num> vector,
          {required String field, int? limit, Filter? filter, CallOptions options = const CallOptions()}) =>
      _client.vector
          .search(name, vector, field: field, limit: limit, filter: filter, namespace: namespace, options: options);

  Future<Map<String, dynamic>> drop({CallOptions options = const CallOptions()}) =>
      _client.collections.drop(name, namespace: namespace, options: options);

  Future<Map<String, dynamic>> info({CallOptions options = const CallOptions()}) =>
      _client.collections.get(name, namespace: namespace, options: options);
}
