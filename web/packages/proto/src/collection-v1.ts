// `loams.collection.v1` as one module: the package spans three proto files
// (`collection`, `document`, `query`), and the SDK facade imports a package's
// types from one specifier (`@loams/proto/collection-v1`), as `./admin.ts`
// does for `loams.admin.v1`. Each file stays importable on its own path too.

export * from './gen/loams/collection/v1/collection_pb.js';
export * from './gen/loams/collection/v1/document_pb.js';
export * from './gen/loams/collection/v1/query_pb.js';
