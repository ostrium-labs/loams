// @loams/client: the Loams SDK for TypeScript and JavaScript.
//
//   import { Loams, apiKey } from '@loams/client';
//
//   const loams = new Loams({ endpoint: 'https://acme.loams.dev', auth: apiKey(key) });
//   const info = await loams.instance.getInstance({});
//   await loams.system.guard('live');            // no RPC once cached
//   for await (const t of loams.live.watch(request, { resume })) {}
//
// Design §44 §7: one client object with namespaced modules, generated from the
// `loams.options.v1` annotations on the protos, over the unified Connect API
// on one port. `@loams/client/node` adds the HTTP/2 gRPC transport.
//
// What a caller imports from here: the `Loams` client, the module types the
// generator produced, the typed error hierarchy, the token sources, and the
// reason registry as a type.

export {
  Loams,
  type AdminNamespace,
  type CollectionsNamespace,
  type DocumentsNamespace,
  type LiveNamespace,
  type NamespacesNamespace,
  type SearchNamespace,
  type SqlNamespace,
  type StreamsNamespace,
  type LoamsEnvironment,
  type LoamsModules,
  type LoamsOptions,
} from './loams.js';

// The app-building layer (SDKP): the builders, the collection handle and the
// hand-written conveniences the module objects carry.
export { Schema, type DynamicMapping, type FieldOptions, type TextOptions as SchemaTextOptions } from './builders/schema.js';
export {
  Filter,
  idValue,
  toQuery,
  type Bounds,
  type DocId,
  type FilterJson,
  type Fuzziness,
  type MatchOptions,
  type MultiMatchOptions,
  type QueryStringOptions,
} from './builders/filter.js';
export {
  Fusion,
  Search,
  Sort,
  consistencyJson,
  dbsf,
  rrf,
  weighted,
  type ConsistencyLevel,
  type ExpandOptions,
  type FusionJson,
  type HighlightOptions,
  type SelectOptions,
  type SortJson,
  type SortValue,
  type SparseOptions,
  type TextOptions,
  type VectorOptions,
} from './builders/search.js';
export { CollectionHandle, type HandleModules } from './handle.js';
export {
  WriteOps,
  attachAppCalls,
  documentCalls,
  documentIdJson,
  searchCalls,
  sqlCalls,
  sqlRecords,
  streamCalls,
  type DocumentInput,
  type PatchChange,
  type ScrollOptions,
  type SearchShorthandOptions,
  type SqlAnswer,
  type SqlRecordsOptions,
  type Target,
} from './app.js';
export {
  DEFAULT_CHUNK_BYTES,
  DEFAULT_CHUNK_ROWS,
  writeBatch,
  type BatchOptions,
  type BatchResult,
  type ChunkWriter,
} from './runtime/batch.js';
export {
  SUBSCRIBE_MAX_BACKOFF_MS,
  SUBSCRIBE_MIN_BACKOFF_MS,
  subscribe,
  type OffsetRecord,
  type RecordFetcher,
  type SubscribeRequest,
  type Subscription,
} from './runtime/subscribe.js';
export {
  SystemApi,
  type Catalogue,
  type SystemConfig,
  type VersionReport,
} from './system.js';

// The generated facade: the module interfaces, the binding table, the proto
// revision, and the reason registry as a type. Everything here is written by
// `buf generate`; do not hand-edit it.
export {
  FEATURE_NOT_IN_VARIANT,
  MODULES,
  PROTO_PACKAGES,
  PROTO_REV,
  REASON_CODES,
  REASONS,
  SERVICE_DESCRIPTORS,
  type CallBinding,
  type CollectionsModule,
  type DocumentsModule,
  type GraphModule,
  type InstanceModule,
  type NamespacesModule,
  type SearchModule,
  type VectorModule,
  type LiveModule,
  type ModuleBinding,
  type Reason,
  type TablesModule,
} from './gen/facade.js';

// The runtime, for a caller who composes rather than uses.
export {
  CONSISTENCY_TOKEN_HEADER,
  CallInvoker,
  DEFAULT_TIMEOUT_MS,
  callWithRetry,
  withIdempotencyKey,
  type Attempt,
  type CallOptions,
  type ConsistencyOptions,
  type ConsistencyTokenStore,
  type InvokerDefaults,
  type RetryPlan,
  type Send,
} from './runtime/call.js';
export {
  ConsistencySession,
  ConsistencyToken,
  TOKEN_PREFIX,
  isConsistencyToken,
  type TokenPosition,
} from './runtime/consistency.js';
export {
  AlreadyExistsError,
  AbortedError,
  BatchError,
  CLIENT_REASONS,
  DeadlineExceededError,
  FailedPreconditionError,
  FeatureNotInVariantError,
  InternalError,
  InvalidArgumentError,
  LoamsError,
  NotFoundError,
  PermissionDeniedError,
  ResourceExhaustedError,
  TokenExpiredError,
  UnauthenticatedError,
  UnavailableError,
  UnimplementedError,
  errorInfo,
  isLoamsError,
  type ClientReason,
  toLoamsError,
  type ErrorInfoShape,
} from './runtime/errors.js';
export { paginate, pageFields, type PageFetcher, type PageRequestOptions } from './runtime/pagination.js';
export {
  BASE_DELAY_MS,
  DEFAULT_MAX_RETRIES,
  MAX_DELAY_MS,
  MAX_SERVER_DELAY_MS,
  backoffMs,
  isRetryableCode,
  shouldRetry,
  sleep,
} from './runtime/retry.js';
export { watch, type CursorOf, type ResumeOptions, type StreamOpener } from './runtime/streams.js';
export {
  apiKey,
  envToken,
  oidcExchange,
  refreshing,
  staticToken,
  type TokenSource,
} from './runtime/token-source.js';
export {
  JWT_BEARER_GRANT,
  MACHINE_KEY_ASSERTION_TTL_S,
  MACHINE_KEY_REFRESH_EARLY_S,
  machineKey,
  type MachineKeyFile,
  type MachineKeyOptions,
} from './runtime/machine-key.js';
export {
  createLoamsTransport,
  headersInterceptor,
  type TransportOptions,
} from './runtime/transports.js';
export { uuidv7, uuidv7Time } from './runtime/uuidv7.js';
