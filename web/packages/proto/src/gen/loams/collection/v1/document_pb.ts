// Documents: the write, the three reads and the two filter writes (design §44
// §5.1, D602; API1 Task 3).
//
// This package replaces the M1.2/M1.5 native REST routes under
// `/v1/namespaces/{ns}/collections/{c}/documents` with RPCs on the one port
// (design §44 §4), and the plan's rulings are visible on the wire:
//
// - **Names are fields, not path segments.** A Connect path is
//   `/<package>.<Service>/<Method>`, so `namespace` and `collection` travel
//   in every request message and one path serves every collection (plan
//   ruling 4). A collection is named by its name or by an alias, exactly as
//   the REST route named it.
// - **proto3 JSON is the wire form**: field names are `lowerCamelCase`
//   (`report_existence` is `reportExistence`, `max_rows` is `maxRows`), an
//   enum answers its proto name in `UPPER_SNAKE`, a 64-bit integer answers a
//   decimal string (so it stays lossless in JavaScript), and a field at its
//   default is *absent* rather than `0`, `false`, `""` or `[]`.
// - **Unknown fields are ignored, not refused.** proto3 JSON says to skip a
//   field the message does not declare, so the REST routes' `deny_unknown_fields`
//   refusal (`{"patch": {}}` on a delete) has no spelling here. What a caller
//   can still be refused for is a *known* field of the wrong JSON type, and the
//   codec answers `invalid_argument` for it before a handler runs.
// - **Every failure carries a reason.** A failed RPC carries one
//   `loams.errors.v1.ErrorInfo` in its details whose `reason` is registered
//   in `docs/api/reasons.md`; a caller branches on the reason, never on the
//   message (D611). `index` names the op of a rejected write and `matched`
//   and `limit` the numbers a filter write refused on, so neither needs the
//   prose. Every reason this package raises is a registry row already.
//
// ## `WriteDocuments` is the first RPC that needs an idempotency key
//
// Every RPC in `loams.collection.v1` is idempotent by construction, so none
// of them carries one (plan ruling 2.4). A write is not: a client that
// retries after a lost answer would write twice. `idempotency_key` makes the
// retry a *replay* — the same token, the same per-op results and the same
// positions, which is what makes the operation safe to repeat at all (D610).
// The window is the immediate retry, and it is documented as such in
// `crates/loams/src/api/connect_documents.rs`; a caller that needs a longer
// window than the server keeps gets a `not_found`-shaped miss (a fresh write),
// not a wrong answer.
//
// ## The payload is JSON, deliberately
//
// A document's `source` is `google.protobuf.Struct`, and its vectors are
// `map<string, google.protobuf.Value>`. A document's payload is an open
// document: a new source key per application, and a vector shape no proto can
// enumerate (dense, sparse, multi-vector, learned). A `Struct` and a `Value`
// keep the native REST route's document JSON exactly, so moving a caller onto
// this RPC changes the envelope it posts in, not the document it posts — the
// same call `loams.collection.v1` made for a schema (plan ruling 2.1). The
// one cost is a `null`: a patch removes a vector by writing
// `{"embedding": null}`, and that is representable at all only because the
// value is a `Value` rather than a `ListValue`.
//
// `Document.fields` is a `Value` per name for the same reason: a field value
// is one of a string, a bool, an integer, a float or `{"date": "…"}` (Ruling
// 9), and the REST JSON already spells all five.
//
// ## A document id is a typed oneof
//
// `DocumentId` is a oneof of `uint` / `string` / `uuid`, not a
// `google.protobuf.Value`. A `Value` holds every number in a `double`, so a
// `u64` id would come back as `18446744073709552000` — a different document.
// The three arms are the three `PrimaryKey` variants, and `uuid` keeps the
// REST spelling `{"uuid": "0190f5c4-…"}` verbatim, which is why it is a
// separate arm rather than the `string` one.
//
// The proto3 JSON of a oneof is `{"uint": "1"}` / `{"string": "k-str"}` /
// `{"uuid": "…"}`: a bare JSON `1` has no oneof spelling, so each arm is named
// on the wire. Setting two arms at once is a decode failure.
//
// ## `filter` is a `Value`, not the typed IR
//
// The typed filter IR is API1 Task 4. Until it lands, `filter` is the native
// query JSON verbatim (`{"term": {"field": "tenant", "value": "c"}}`,
// `"match_all"`), which is what the REST route took and what a caller already
// has. `select` is a `Struct` for the same reason and because a projection's
// `source` has a JSON object form (`{"include": [], "exclude": []}`) that a
// `string` cannot hold.
//
// ## Where the tokens and the backlog live
//
// A write answers its consistency token in `token` and in the
// `loams-consistency-token` response header; a read answers `readToken` and
// the same header. The unapplied-data backlog (`loams-unapplied-records`,
// `loams-unapplied-bytes`) stays response headers, because that is the REST
// behaviour and a response half is headers by rule (design §44 §7.4). The one
// request header that stays is `loams-backpressure: off`, the write-budget
// override (M1.3 Task 15 rule 5): it is a transport-level switch with no
// message-level meaning, and an unrecognised value is `invalid_argument`.
//
// ## The handlers are thin
//
// Each one calls the same `CollectionService` entry point the REST route it
// replaces calls — `write`, `get_with_token`, `scroll_with_token`,
// `count_with_token`, `delete_by_filter`, `patch_by_filter` — and reuses the
// REST request's own parsing (`op_from_json`, `patch_spec_from_json`) by
// turning the request message back into the JSON those parse, so the two
// surfaces cannot drift. The behaviour is the behaviour
// `crates/loams/tests/it/` already pins; the REST routes stay until Task 9.
//
// ## Why this is a second file in `loams.collection.v1`, not its own package
//
// §44 §7.2's module catalogue puts both services in one package —
// `loams.collections`, `loams.documents` ⇢ `loams.collection.v1`
// `CollectionService`, `DocumentService` — and §8's `New protos:` sentence
// enumerates `loams.collection.v1` as covering Namespace, Collection,
// **Document** and Query. One package, four concerns, several files: the same
// shape `proto/loams/live/v1/` already uses with its five files. An earlier
// draft gave documents a `loams.document.v1` of their own, which no design
// section mentions; `crates/loams/tests/route_map.rs` now fails if any proto or
// `CATALOGUE` row declares a package §44 does not enumerate.
//
// Splitting the files still matters — `collection.proto` is the schema and scan
// plan, this file is the document data path, and they are read separately — but
// the package is the unit on the wire, in the SDK stubs and in `CATALOGUE`, and
// §44 fixes it.
//
// ## The SDK module, and why no method is in the SDK facade yet
//
// The service carries `loams.options.v1.ModuleOptions` (`loams.documents`),
// which is what §44 §7.3 asks of every public package. Note this file is in
// `loams.collection.v1`, so the module a caller reaches is named by the
// `module` option here, not by the package name.
//
// No method carries `FacadeOptions`, which is plan ruling 2.5's shape again:
// a `facade` option makes `protoc-gen-loams-facade` emit a call, and the call
// imports the package's message types from the generated stubs, which means
// every SDK mirror has to generate `loams.collection.v1` first. That is SDK1's
// stub task. Sharing the package also means the generated facades barely move:
// `PROTO_PACKAGES` already listed `loams.collection.v1`, so what changed is that
// a `loams.document.v1` line went away. `facade.ts`, `facade.py`, `facade.rs`
// and the three goldens are regenerated with it.

// @generated by protoc-gen-es v2.16.0 with parameter "target=ts,import_extension=js"
// @generated from file loams/collection/v1/document.proto (package loams.collection.v1, syntax proto3)
/* eslint-disable */

import type { GenEnum, GenFile, GenMessage, GenService } from "@bufbuild/protobuf/codegenv2";
import { enumDesc, fileDesc, messageDesc, serviceDesc } from "@bufbuild/protobuf/codegenv2";
import type { Value } from "@bufbuild/protobuf/wkt";
import { file_google_protobuf_struct } from "@bufbuild/protobuf/wkt";
import { file_loams_options_v1_options } from "../../options/v1/options_pb.js";
import type { JsonObject, Message } from "@bufbuild/protobuf";

/**
 * Describes the file loams/collection/v1/document.proto.
 */
export const file_loams_collection_v1_document: GenFile = /*@__PURE__*/
  fileDesc("CiJsb2Ftcy9jb2xsZWN0aW9uL3YxL2RvY3VtZW50LnByb3RvEhNsb2Ftcy5jb2xsZWN0aW9uLnYxIpwBChVXcml0ZURvY3VtZW50c1JlcXVlc3QSEQoJbmFtZXNwYWNlGAEgASgJEhIKCmNvbGxlY3Rpb24YAiABKAkSKQoDb3BzGAMgAygLMhwubG9hbXMuY29sbGVjdGlvbi52MS5Xcml0ZU9wEhgKEHJlcG9ydF9leGlzdGVuY2UYBCABKAgSFwoPaWRlbXBvdGVuY3lfa2V5GAUgASgJIvMBChZXcml0ZURvY3VtZW50c1Jlc3BvbnNlEg0KBXRva2VuGAEgASgJEi4KB3Jlc3VsdHMYAiADKA4yHS5sb2Ftcy5jb2xsZWN0aW9uLnYxLk9wUmVzdWx0EjIKCXBvc2l0aW9ucxgDIAMoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuT3BQb3NpdGlvbhIeChF1bmFwcGxpZWRfcmVjb3JkcxgEIAEoBEgAiAEBEhwKD3VuYXBwbGllZF9ieXRlcxgFIAEoBEgBiAEBQhQKEl91bmFwcGxpZWRfcmVjb3Jkc0ISChBfdW5hcHBsaWVkX2J5dGVzIqkBCgdXcml0ZU9wEjQKBnVwc2VydBgBIAEoCzIiLmxvYW1zLmNvbGxlY3Rpb24udjEuV3JpdGVEb2N1bWVudEgAEjUKBmRlbGV0ZRgCIAEoCzIjLmxvYW1zLmNvbGxlY3Rpb24udjEuRGVsZXRlRG9jdW1lbnRIABIrCgVwYXRjaBgDIAEoCzIaLmxvYW1zLmNvbGxlY3Rpb24udjEuUGF0Y2hIAEIECgJvcCKNAwoNV3JpdGVEb2N1bWVudBIrCgJpZBgBIAEoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuRG9jdW1lbnRJZBInCgZzb3VyY2UYAiABKAsyFy5nb29nbGUucHJvdG9idWYuU3RydWN0EkAKB3ZlY3RvcnMYAyADKAsyLy5sb2Ftcy5jb2xsZWN0aW9uLnYxLldyaXRlRG9jdW1lbnQuVmVjdG9yc0VudHJ5Ek0KDnNwYXJzZV92ZWN0b3JzGAQgAygLMjUubG9hbXMuY29sbGVjdGlvbi52MS5Xcml0ZURvY3VtZW50LlNwYXJzZVZlY3RvcnNFbnRyeRpGCgxWZWN0b3JzRW50cnkSCwoDa2V5GAEgASgJEiUKBXZhbHVlGAIgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlOgI4ARpNChJTcGFyc2VWZWN0b3JzRW50cnkSCwoDa2V5GAEgASgJEiYKBXZhbHVlGAIgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdDoCOAEiPQoORGVsZXRlRG9jdW1lbnQSKwoCaWQYASABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLkRvY3VtZW50SWQizAMKBVBhdGNoEisKAmlkGAEgASgLMh8ubG9hbXMuY29sbGVjdGlvbi52MS5Eb2N1bWVudElkEgwKBG1vZGUYAiABKAkSJwoGc291cmNlGAMgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdBITCgtkZWxldGVfa2V5cxgEIAMoCRI4Cgd2ZWN0b3JzGAUgAygLMicubG9hbXMuY29sbGVjdGlvbi52MS5QYXRjaC5WZWN0b3JzRW50cnkSRQoOc3BhcnNlX3ZlY3RvcnMYBiADKAsyLS5sb2Ftcy5jb2xsZWN0aW9uLnYxLlBhdGNoLlNwYXJzZVZlY3RvcnNFbnRyeRIyCgZ1cHNlcnQYByABKAsyIi5sb2Ftcy5jb2xsZWN0aW9uLnYxLldyaXRlRG9jdW1lbnQaRgoMVmVjdG9yc0VudHJ5EgsKA2tleRgBIAEoCRIlCgV2YWx1ZRgCIAEoCzIWLmdvb2dsZS5wcm90b2J1Zi5WYWx1ZToCOAEaTQoSU3BhcnNlVmVjdG9yc0VudHJ5EgsKA2tleRgBIAEoCRImCgV2YWx1ZRgCIAEoCzIXLmdvb2dsZS5wcm90b2J1Zi5TdHJ1Y3Q6AjgBIkQKCkRvY3VtZW50SWQSDgoEdWludBgBIAEoBEgAEhAKBnN0cmluZxgCIAEoCUgAEg4KBHV1aWQYAyABKAlIAEIECgJpZCI/CgpPcFBvc2l0aW9uEhEKCXBhcnRpdGlvbhgBIAEoDRITCgZzZXFfbm8YAiABKARIAIgBAUIJCgdfc2VxX25vIsoBChNHZXREb2N1bWVudHNSZXF1ZXN0EhEKCW5hbWVzcGFjZRgBIAEoCRISCgpjb2xsZWN0aW9uGAIgASgJEiwKA2lkcxgDIAMoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuRG9jdW1lbnRJZBInCgZzZWxlY3QYBCABKAsyFy5nb29nbGUucHJvdG9idWYuU3RydWN0EjUKC2NvbnNpc3RlbmN5GAUgASgLMiAubG9hbXMuY29sbGVjdGlvbi52MS5Db25zaXN0ZW5jeSJcChRHZXREb2N1bWVudHNSZXNwb25zZRIwCglkb2N1bWVudHMYASADKAsyHS5sb2Ftcy5jb2xsZWN0aW9uLnYxLkRvY3VtZW50EhIKCnJlYWRfdG9rZW4YAiABKAkiygIKFlNjcm9sbERvY3VtZW50c1JlcXVlc3QSEQoJbmFtZXNwYWNlGAEgASgJEhIKCmNvbGxlY3Rpb24YAiABKAkSJgoGZmlsdGVyGAMgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlEi4KBWFmdGVyGAQgASgLMh8ubG9hbXMuY29sbGVjdGlvbi52MS5Eb2N1bWVudElkEhIKBWxpbWl0GAUgASgNSACIAQESJwoGc2VsZWN0GAYgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdBI1Cgtjb25zaXN0ZW5jeRgHIAEoCzIgLmxvYW1zLmNvbGxlY3Rpb24udjEuQ29uc2lzdGVuY3kSMwoKcGFnZV90b2tlbhgIIAEoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuRG9jdW1lbnRJZEIICgZfbGltaXQijgEKF1Njcm9sbERvY3VtZW50c1Jlc3BvbnNlEjAKCWRvY3VtZW50cxgBIAMoCzIdLmxvYW1zLmNvbGxlY3Rpb24udjEuRG9jdW1lbnQSLQoEbmV4dBgCIAEoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuRG9jdW1lbnRJZBISCgpyZWFkX3Rva2VuGAMgASgJIp0BChVDb3VudERvY3VtZW50c1JlcXVlc3QSEQoJbmFtZXNwYWNlGAEgASgJEhIKCmNvbGxlY3Rpb24YAiABKAkSJgoGZmlsdGVyGAMgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlEjUKC2NvbnNpc3RlbmN5GAQgASgLMiAubG9hbXMuY29sbGVjdGlvbi52MS5Db25zaXN0ZW5jeSJKChZDb3VudERvY3VtZW50c1Jlc3BvbnNlEhIKBWNvdW50GAEgASgESACIAQESEgoKcmVhZF90b2tlbhgCIAEoCUIICgZfY291bnQikAIKFURlbGV0ZUJ5RmlsdGVyUmVxdWVzdBIRCgluYW1lc3BhY2UYASABKAkSEgoKY29sbGVjdGlvbhgCIAEoCRImCgZmaWx0ZXIYAyABKAsyFi5nb29nbGUucHJvdG9idWYuVmFsdWUSFQoIbWF4X3Jvd3MYBCABKARIAIgBARIVCg1hbGxvd19wYXJ0aWFsGAUgASgIEjYKBmN1cnNvchgGIAEoCzImLmxvYW1zLmNvbGxlY3Rpb24udjEuRmlsdGVyV3JpdGVDdXJzb3ISNQoLY29uc2lzdGVuY3kYByABKAsyIC5sb2Ftcy5jb2xsZWN0aW9uLnYxLkNvbnNpc3RlbmN5QgsKCV9tYXhfcm93cyK6AgoUUGF0Y2hCeUZpbHRlclJlcXVlc3QSEQoJbmFtZXNwYWNlGAEgASgJEhIKCmNvbGxlY3Rpb24YAiABKAkSJgoGZmlsdGVyGAMgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlEikKBXBhdGNoGAQgASgLMhoubG9hbXMuY29sbGVjdGlvbi52MS5QYXRjaBIVCghtYXhfcm93cxgFIAEoBEgAiAEBEhUKDWFsbG93X3BhcnRpYWwYBiABKAgSNgoGY3Vyc29yGAcgASgLMiYubG9hbXMuY29sbGVjdGlvbi52MS5GaWx0ZXJXcml0ZUN1cnNvchI1Cgtjb25zaXN0ZW5jeRgIIAEoCzIgLmxvYW1zLmNvbGxlY3Rpb24udjEuQ29uc2lzdGVuY3lCCwoJX21heF9yb3dzItUCChNGaWx0ZXJXcml0ZVJlc3BvbnNlEhQKB21hdGNoZWQYASABKARIAIgBARIVCghhZmZlY3RlZBgCIAEoBEgBiAEBEhQKB3dyaXR0ZW4YAyABKARIAogBARIUCgdiYXRjaGVzGAQgASgESAOIAQESFgoOcm93c19yZW1haW5pbmcYBSABKAgSNgoGY3Vyc29yGAYgASgLMiYubG9hbXMuY29sbGVjdGlvbi52MS5GaWx0ZXJXcml0ZUN1cnNvchINCgV0b2tlbhgHIAEoCRIlCgNwaW4YCCABKAsyGC5sb2Ftcy5jb2xsZWN0aW9uLnYxLlBpbhIbCg5yZXRyeV9hZnRlcl9tcxgJIAEoBEgEiAEBQgoKCF9tYXRjaGVkQgsKCV9hZmZlY3RlZEIKCghfd3JpdHRlbkIKCghfYmF0Y2hlc0IRCg9fcmV0cnlfYWZ0ZXJfbXMibAoRRmlsdGVyV3JpdGVDdXJzb3ISLgoFYWZ0ZXIYASABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLkRvY3VtZW50SWQSGAoQbWFuaWZlc3RfdmVyc2lvbhgCIAEoBBINCgV0b2tlbhgDIAEoCSKzBAoIRG9jdW1lbnQSKwoCaWQYASABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLkRvY3VtZW50SWQSJwoGc291cmNlGAIgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdBI7Cgd2ZWN0b3JzGAMgAygLMioubG9hbXMuY29sbGVjdGlvbi52MS5Eb2N1bWVudC5WZWN0b3JzRW50cnkSSAoOc3BhcnNlX3ZlY3RvcnMYBCADKAsyMC5sb2Ftcy5jb2xsZWN0aW9uLnYxLkRvY3VtZW50LlNwYXJzZVZlY3RvcnNFbnRyeRI5CgZmaWVsZHMYBSADKAsyKS5sb2Ftcy5jb2xsZWN0aW9uLnYxLkRvY3VtZW50LkZpZWxkc0VudHJ5EhMKBnNlcV9ubxgGIAEoBEgAiAEBEhEKCXBhcnRpdGlvbhgHIAEoDRpGCgxWZWN0b3JzRW50cnkSCwoDa2V5GAEgASgJEiUKBXZhbHVlGAIgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlOgI4ARpNChJTcGFyc2VWZWN0b3JzRW50cnkSCwoDa2V5GAEgASgJEiYKBXZhbHVlGAIgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdDoCOAEaRQoLRmllbGRzRW50cnkSCwoDa2V5GAEgASgJEiUKBXZhbHVlGAIgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlOgI4AUIJCgdfc2VxX25vInkKC0NvbnNpc3RlbmN5EjEKCWZyZXNobmVzcxgBIAEoDjIeLmxvYW1zLmNvbGxlY3Rpb24udjEuRnJlc2huZXNzEhAKCGF0X2xlYXN0GAIgASgJEiUKA3BpbhgDIAEoCzIYLmxvYW1zLmNvbGxlY3Rpb24udjEuUGluIi4KA1BpbhIYChBtYW5pZmVzdF92ZXJzaW9uGAEgASgEEg0KBXRva2VuGAIgASgJKq8BCghPcFJlc3VsdBIZChVPUF9SRVNVTFRfVU5TUEVDSUZJRUQQABIVChFPUF9SRVNVTFRfQ1JFQVRFRBABEhUKEU9QX1JFU1VMVF9VUERBVEVEEAISFQoRT1BfUkVTVUxUX0RFTEVURUQQAxIXChNPUF9SRVNVTFRfTk9UX0ZPVU5EEAQSEgoOT1BfUkVTVUxUX05PT1AQBRIWChJPUF9SRVNVTFRfQUNDRVBURUQQBipUCglGcmVzaG5lc3MSGQoVRlJFU0hORVNTX1VOU1BFQ0lGSUVEEAASFAoQRlJFU0hORVNTX1NUUk9ORxABEhYKEkZSRVNITkVTU19FVkVOVFVBTBACMvsFCg9Eb2N1bWVudFNlcnZpY2USawoOV3JpdGVEb2N1bWVudHMSKi5sb2Ftcy5jb2xsZWN0aW9uLnYxLldyaXRlRG9jdW1lbnRzUmVxdWVzdBorLmxvYW1zLmNvbGxlY3Rpb24udjEuV3JpdGVEb2N1bWVudHNSZXNwb25zZSIAEmgKDEdldERvY3VtZW50cxIoLmxvYW1zLmNvbGxlY3Rpb24udjEuR2V0RG9jdW1lbnRzUmVxdWVzdBopLmxvYW1zLmNvbGxlY3Rpb24udjEuR2V0RG9jdW1lbnRzUmVzcG9uc2UiA5ACARJxCg9TY3JvbGxEb2N1bWVudHMSKy5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNjcm9sbERvY3VtZW50c1JlcXVlc3QaLC5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNjcm9sbERvY3VtZW50c1Jlc3BvbnNlIgOQAgESbgoOQ291bnREb2N1bWVudHMSKi5sb2Ftcy5jb2xsZWN0aW9uLnYxLkNvdW50RG9jdW1lbnRzUmVxdWVzdBorLmxvYW1zLmNvbGxlY3Rpb24udjEuQ291bnREb2N1bWVudHNSZXNwb25zZSIDkAIBEmgKDkRlbGV0ZUJ5RmlsdGVyEioubG9hbXMuY29sbGVjdGlvbi52MS5EZWxldGVCeUZpbHRlclJlcXVlc3QaKC5sb2Ftcy5jb2xsZWN0aW9uLnYxLkZpbHRlcldyaXRlUmVzcG9uc2UiABJmCg1QYXRjaEJ5RmlsdGVyEikubG9hbXMuY29sbGVjdGlvbi52MS5QYXRjaEJ5RmlsdGVyUmVxdWVzdBooLmxvYW1zLmNvbGxlY3Rpb24udjEuRmlsdGVyV3JpdGVSZXNwb25zZSIAGlyKtRhYCglkb2N1bWVudHMSS0RvY3VtZW50czogd3JpdGUsIGdldCwgc2Nyb2xsLCBjb3VudCwgZGVsZXRlIGJ5IGZpbHRlciBhbmQgcGF0Y2ggYnkgZmlsdGVyLmIGcHJvdG8z", [file_google_protobuf_struct, file_loams_options_v1_options]);

/**
 * @generated from message loams.collection.v1.WriteDocumentsRequest
 */
export type WriteDocumentsRequest = Message<"loams.collection.v1.WriteDocumentsRequest"> & {
  /**
   * @generated from field: string namespace = 1;
   */
  namespace: string;

  /**
   * A collection name or an alias.
   *
   * @generated from field: string collection = 2;
   */
  collection: string;

  /**
   * Applied in order, and all of them or none of them.
   *
   * @generated from field: repeated loams.collection.v1.WriteOp ops = 3;
   */
  ops: WriteOp[];

  /**
   * Whether `results` reports what each op did to an existing document
   * (`created`, `updated`, `deleted`, `not_found`, `noop`) rather than
   * `accepted`. Absent is `false`, which is the cheaper answer.
   *
   * @generated from field: bool report_existence = 4;
   */
  reportExistence: boolean;

  /**
   * The key that makes a retry a replay (D610). Absent means the caller is not
   * retrying and every call is a new write.
   *
   * @generated from field: string idempotency_key = 5;
   */
  idempotencyKey: string;
};

/**
 * Describes the message loams.collection.v1.WriteDocumentsRequest.
 * Use `create(WriteDocumentsRequestSchema)` to create a new message.
 */
export const WriteDocumentsRequestSchema: GenMessage<WriteDocumentsRequest> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 0);

/**
 * @generated from message loams.collection.v1.WriteDocumentsResponse
 */
export type WriteDocumentsResponse = Message<"loams.collection.v1.WriteDocumentsResponse"> & {
  /**
   * The writes' consistency token: at least these writes are acknowledged.
   * The same value is the `loams-consistency-token` response header.
   *
   * @generated from field: string token = 1;
   */
  token: string;

  /**
   * One per op, in request order.
   *
   * @generated from field: repeated loams.collection.v1.OpResult results = 2;
   */
  results: OpResult[];

  /**
   * One per op: where it was appended. An op that was not placed answers an
   * entry with no fields, which is how a `noop` reports itself.
   *
   * @generated from field: repeated loams.collection.v1.OpPosition positions = 3;
   */
  positions: OpPosition[];

  /**
   * The unapplied-data backlog measured at admission, before this write's own
   * records. Also the `loams-unapplied-records` / `loams-unapplied-bytes`
   * response headers, which is where a non-Connect reader reads them.
   *
   * @generated from field: optional uint64 unapplied_records = 4;
   */
  unappliedRecords?: bigint | undefined;

  /**
   * @generated from field: optional uint64 unapplied_bytes = 5;
   */
  unappliedBytes?: bigint | undefined;
};

/**
 * Describes the message loams.collection.v1.WriteDocumentsResponse.
 * Use `create(WriteDocumentsResponseSchema)` to create a new message.
 */
export const WriteDocumentsResponseSchema: GenMessage<WriteDocumentsResponse> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 1);

/**
 * One document change. A proto oneof, so the JSON is the REST route's
 * `{"upsert": {…}}` / `{"delete": {…}}` / `{"patch": {…}}` unchanged, and an
 * op that is none of the three is `invalid_argument`.
 *
 * @generated from message loams.collection.v1.WriteOp
 */
export type WriteOp = Message<"loams.collection.v1.WriteOp"> & {
  /**
   * @generated from oneof loams.collection.v1.WriteOp.op
   */
  op: {
    /**
     * Writes the document under its id, replacing it if it is there.
     *
     * @generated from field: loams.collection.v1.WriteDocument upsert = 1;
     */
    value: WriteDocument;
    case: "upsert";
  } | {
    /**
     * Removes the document. Removing one that is not there is a `not_found`,
     * which is only reported with `report_existence`.
     *
     * @generated from field: loams.collection.v1.DeleteDocument delete = 2;
     */
    value: DeleteDocument;
    case: "delete";
  } | {
    /**
     * Changes part of a document, and may create one (see `Patch.upsert`).
     *
     * @generated from field: loams.collection.v1.Patch patch = 3;
     */
    value: Patch;
    case: "patch";
  } | { case: undefined; value?: undefined };
};

/**
 * Describes the message loams.collection.v1.WriteOp.
 * Use `create(WriteOpSchema)` to create a new message.
 */
export const WriteOpSchema: GenMessage<WriteOp> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 2);

/**
 * @generated from message loams.collection.v1.WriteDocument
 */
export type WriteDocument = Message<"loams.collection.v1.WriteDocument"> & {
  /**
   * @generated from field: loams.collection.v1.DocumentId id = 1;
   */
  id?: DocumentId | undefined;

  /**
   * The document's payload, as the native REST route's `_source` JSON. Absent
   * is an empty document.
   *
   * @generated from field: google.protobuf.Struct source = 2;
   */
  source?: JsonObject | undefined;

  /**
   * Dense vectors by name, as the REST route's JSON (`[1.0, 0.0, 0.0]`).
   *
   * @generated from field: map<string, google.protobuf.Value> vectors = 3;
   */
  vectors: { [key: string]: Value };

  /**
   * Sparse vectors by name, as the REST route's JSON (`{"indices": [1],
   * "values": [1.0]}`).
   *
   * @generated from field: map<string, google.protobuf.Struct> sparse_vectors = 4;
   */
  sparseVectors: { [key: string]: JsonObject };
};

/**
 * Describes the message loams.collection.v1.WriteDocument.
 * Use `create(WriteDocumentSchema)` to create a new message.
 */
export const WriteDocumentSchema: GenMessage<WriteDocument> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 3);

/**
 * @generated from message loams.collection.v1.DeleteDocument
 */
export type DeleteDocument = Message<"loams.collection.v1.DeleteDocument"> & {
  /**
   * @generated from field: loams.collection.v1.DocumentId id = 1;
   */
  id?: DocumentId | undefined;
};

/**
 * Describes the message loams.collection.v1.DeleteDocument.
 * Use `create(DeleteDocumentSchema)` to create a new message.
 */
export const DeleteDocumentSchema: GenMessage<DeleteDocument> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 4);

/**
 * One document's change. `mode` and every other spelling here are the REST
 * route's, because a reader switches on them verbatim (the same call
 * `loams.collection.v1` made for `ScanColumn.role`).
 *
 * @generated from message loams.collection.v1.Patch
 */
export type Patch = Message<"loams.collection.v1.Patch"> & {
  /**
   * @generated from field: loams.collection.v1.DocumentId id = 1;
   */
  id?: DocumentId | undefined;

  /**
   * `merge_deep` (the default), `merge_top` or `replace`. A string rather than
   * an enum so the REST spelling survives verbatim; any other value is
   * `invalid_argument`.
   *
   * @generated from field: string mode = 2;
   */
  mode: string;

  /**
   * The keys to merge. `merge_deep` merges nested objects, `merge_top`
   * replaces them and `replace` replaces the document.
   *
   * @generated from field: google.protobuf.Struct source = 3;
   */
  source?: JsonObject | undefined;

  /**
   * The top-level source keys to remove.
   *
   * @generated from field: repeated string delete_keys = 4;
   */
  deleteKeys: string[];

  /**
   * Vectors to set, by name. A `null` value **removes** the vector, which is
   * why the value is a `Value` and not a `ListValue` (see the header).
   *
   * @generated from field: map<string, google.protobuf.Value> vectors = 5;
   */
  vectors: { [key: string]: Value };

  /**
   * Sparse vectors to set, by name; a `null` value removes one.
   *
   * @generated from field: map<string, google.protobuf.Struct> sparse_vectors = 6;
   */
  sparseVectors: { [key: string]: JsonObject };

  /**
   * The document to write when the patched document is not there. Absent
   * leaves a missing document missing, which is what a `patch_by_filter`
   * never does (it patches what the filter matched).
   *
   * @generated from field: loams.collection.v1.WriteDocument upsert = 7;
   */
  upsert?: WriteDocument | undefined;
};

/**
 * Describes the message loams.collection.v1.Patch.
 * Use `create(PatchSchema)` to create a new message.
 */
export const PatchSchema: GenMessage<Patch> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 5);

/**
 * One document's id, as the three `PrimaryKey` variants (see the header).
 *
 * @generated from message loams.collection.v1.DocumentId
 */
export type DocumentId = Message<"loams.collection.v1.DocumentId"> & {
  /**
   * @generated from oneof loams.collection.v1.DocumentId.id
   */
  id: {
    /**
     * An id in `0..=2^64−1`, as a decimal string in JSON.
     *
     * @generated from field: uint64 uint = 1;
     */
    value: bigint;
    case: "uint";
  } | {
    /**
     * @generated from field: string string = 2;
     */
    value: string;
    case: "string";
  } | {
    /**
     * The REST spelling, `8-4-4-4-12` in any case.
     *
     * @generated from field: string uuid = 3;
     */
    value: string;
    case: "uuid";
  } | { case: undefined; value?: undefined };
};

/**
 * Describes the message loams.collection.v1.DocumentId.
 * Use `create(DocumentIdSchema)` to create a new message.
 */
export const DocumentIdSchema: GenMessage<DocumentId> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 6);

/**
 * Where one op was appended.
 *
 * An op that was not placed answers an entry with **no `seqNo` at all**, which
 * is why `seq_no` is `optional` and not a plain `uint64`: offset 0 is the
 * first record of a partition and a real answer, and proto3 JSON omitting a
 * zero would make it indistinguishable from "this op was not placed".
 *
 * @generated from message loams.collection.v1.OpPosition
 */
export type OpPosition = Message<"loams.collection.v1.OpPosition"> & {
  /**
   * @generated from field: uint32 partition = 1;
   */
  partition: number;

  /**
   * @generated from field: optional uint64 seq_no = 2;
   */
  seqNo?: bigint | undefined;
};

/**
 * Describes the message loams.collection.v1.OpPosition.
 * Use `create(OpPositionSchema)` to create a new message.
 */
export const OpPositionSchema: GenMessage<OpPosition> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 7);

/**
 * @generated from message loams.collection.v1.GetDocumentsRequest
 */
export type GetDocumentsRequest = Message<"loams.collection.v1.GetDocumentsRequest"> & {
  /**
   * @generated from field: string namespace = 1;
   */
  namespace: string;

  /**
   * A collection name or an alias.
   *
   * @generated from field: string collection = 2;
   */
  collection: string;

  /**
   * The documents to get. The answer has one entry per id, in this order.
   *
   * @generated from field: repeated loams.collection.v1.DocumentId ids = 3;
   */
  ids: DocumentId[];

  /**
   * What to return of each document, as the REST route's projection JSON
   * (`{"source": "all" | "none" | {"include": [], "exclude": []},
   * "vectors": [], "fields": []}`). Absent is everything.
   *
   * @generated from field: google.protobuf.Struct select = 4;
   */
  select?: JsonObject | undefined;

  /**
   * @generated from field: loams.collection.v1.Consistency consistency = 5;
   */
  consistency?: Consistency | undefined;
};

/**
 * Describes the message loams.collection.v1.GetDocumentsRequest.
 * Use `create(GetDocumentsRequestSchema)` to create a new message.
 */
export const GetDocumentsRequestSchema: GenMessage<GetDocumentsRequest> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 8);

/**
 * @generated from message loams.collection.v1.GetDocumentsResponse
 */
export type GetDocumentsResponse = Message<"loams.collection.v1.GetDocumentsResponse"> & {
  /**
   * One per requested id, in request order. An entry with no fields is a
   * document that is not there.
   *
   * @generated from field: repeated loams.collection.v1.Document documents = 1;
   */
  documents: Document[];

  /**
   * The state the documents were read at, and a `consistency.at_least` that
   * reads this state again. The same value is the
   * `loams-consistency-token` response header.
   *
   * @generated from field: string read_token = 2;
   */
  readToken: string;
};

/**
 * Describes the message loams.collection.v1.GetDocumentsResponse.
 * Use `create(GetDocumentsResponseSchema)` to create a new message.
 */
export const GetDocumentsResponseSchema: GenMessage<GetDocumentsResponse> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 9);

/**
 * @generated from message loams.collection.v1.ScrollDocumentsRequest
 */
export type ScrollDocumentsRequest = Message<"loams.collection.v1.ScrollDocumentsRequest"> & {
  /**
   * @generated from field: string namespace = 1;
   */
  namespace: string;

  /**
   * A collection name or an alias.
   *
   * @generated from field: string collection = 2;
   */
  collection: string;

  /**
   * The native query JSON, as the REST route takes it. Absent is every
   * document. This stays a `Value` while `query.proto` types the filter IR for
   * `QueryService/Search`: the tests that shipped this RPC send the bare string
   * `"match_all"`, which a typed oneof cannot spell, and `SearchRequest.filter`
   * is where a typed filter lands (API1 Task 4).
   *
   * @generated from field: google.protobuf.Value filter = 3;
   */
  filter?: Value | undefined;

  /**
   * The last document of the previous page; absent starts at the first.
   *
   * @generated from field: loams.collection.v1.DocumentId after = 4;
   */
  after?: DocumentId | undefined;

  /**
   * The page size; absent or `0` is the server's default.
   *
   * @generated from field: optional uint32 limit = 5;
   */
  limit?: number | undefined;

  /**
   * @generated from field: google.protobuf.Struct select = 6;
   */
  select?: JsonObject | undefined;

  /**
   * @generated from field: loams.collection.v1.Consistency consistency = 7;
   */
  consistency?: Consistency | undefined;

  /**
   * The AIP-158 spelling of `after`, and the **same cursor**: a `DocumentId`,
   * encoded exactly as `after` and `ScrollDocumentsResponse.next` encode it, so
   * there is one cursor on the wire and the two fields are interchangeable —
   * a `next` handed straight back as a `page_token` reads the same page. A
   * request that sets both names different cursors is `invalid_argument`.
   *
   * It is an addition rather than a change of shape: plan ruling 2 offers
   * `page_token` as an alternative to server-streaming `ScrollDocuments`, and
   * Q614 — which of the two the method is — is unanswered, so the shipped
   * unary RPC keeps its shape rather than breaking for an open question. True
   * server-streaming, if Q614 says so, is a separate additive RPC.
   *
   * @generated from field: loams.collection.v1.DocumentId page_token = 8;
   */
  pageToken?: DocumentId | undefined;
};

/**
 * Describes the message loams.collection.v1.ScrollDocumentsRequest.
 * Use `create(ScrollDocumentsRequestSchema)` to create a new message.
 */
export const ScrollDocumentsRequestSchema: GenMessage<ScrollDocumentsRequest> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 10);

/**
 * @generated from message loams.collection.v1.ScrollDocumentsResponse
 */
export type ScrollDocumentsResponse = Message<"loams.collection.v1.ScrollDocumentsResponse"> & {
  /**
   * In primary-key order.
   *
   * @generated from field: repeated loams.collection.v1.Document documents = 1;
   */
  documents: Document[];

  /**
   * The id to continue after; absent on the last page.
   *
   * @generated from field: loams.collection.v1.DocumentId next = 2;
   */
  next?: DocumentId | undefined;

  /**
   * @generated from field: string read_token = 3;
   */
  readToken: string;
};

/**
 * Describes the message loams.collection.v1.ScrollDocumentsResponse.
 * Use `create(ScrollDocumentsResponseSchema)` to create a new message.
 */
export const ScrollDocumentsResponseSchema: GenMessage<ScrollDocumentsResponse> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 11);

/**
 * @generated from message loams.collection.v1.CountDocumentsRequest
 */
export type CountDocumentsRequest = Message<"loams.collection.v1.CountDocumentsRequest"> & {
  /**
   * @generated from field: string namespace = 1;
   */
  namespace: string;

  /**
   * A collection name or an alias.
   *
   * @generated from field: string collection = 2;
   */
  collection: string;

  /**
   * @generated from field: google.protobuf.Value filter = 3;
   */
  filter?: Value | undefined;

  /**
   * @generated from field: loams.collection.v1.Consistency consistency = 4;
   */
  consistency?: Consistency | undefined;
};

/**
 * Describes the message loams.collection.v1.CountDocumentsRequest.
 * Use `create(CountDocumentsRequestSchema)` to create a new message.
 */
export const CountDocumentsRequestSchema: GenMessage<CountDocumentsRequest> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 12);

/**
 * @generated from message loams.collection.v1.CountDocumentsResponse
 */
export type CountDocumentsResponse = Message<"loams.collection.v1.CountDocumentsResponse"> & {
  /**
   * How many documents matched, as a decimal string in JSON. `optional` and
   * always set: "nothing matched" is a count, not an absent field, and proto3
   * JSON omitting a zero would make an empty result read as "unanswered".
   *
   * @generated from field: optional uint64 count = 1;
   */
  count?: bigint | undefined;

  /**
   * @generated from field: string read_token = 2;
   */
  readToken: string;
};

/**
 * Describes the message loams.collection.v1.CountDocumentsResponse.
 * Use `create(CountDocumentsResponseSchema)` to create a new message.
 */
export const CountDocumentsResponseSchema: GenMessage<CountDocumentsResponse> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 13);

/**
 * @generated from message loams.collection.v1.DeleteByFilterRequest
 */
export type DeleteByFilterRequest = Message<"loams.collection.v1.DeleteByFilterRequest"> & {
  /**
   * @generated from field: string namespace = 1;
   */
  namespace: string;

  /**
   * A collection name or an alias.
   *
   * @generated from field: string collection = 2;
   */
  collection: string;

  /**
   * The native query JSON, as the REST route takes it. Required: there is no
   * "delete everything" spelling, and `match_all` is the caller's to write.
   *
   * @generated from field: google.protobuf.Value filter = 3;
   */
  filter?: Value | undefined;

  /**
   * The most rows this call may write. Absent is the kind's ceiling; `0` is
   * `invalid_argument`, because a call that may write nothing is a mistake
   * rather than a no-op.
   *
   * @generated from field: optional uint64 max_rows = 4;
   */
  maxRows?: bigint | undefined;

  /**
   * Whether a call matching more than `max_rows` writes what fits and answers a
   * cursor instead of failing. Absent is `false`: a call over its limit fails
   * before writing anything.
   *
   * @generated from field: bool allow_partial = 5;
   */
  allowPartial: boolean;

  /**
   * The `cursor` of the previous partial call, which resumes at the same pin.
   *
   * @generated from field: loams.collection.v1.FilterWriteCursor cursor = 6;
   */
  cursor?: FilterWriteCursor | undefined;

  /**
   * @generated from field: loams.collection.v1.Consistency consistency = 7;
   */
  consistency?: Consistency | undefined;
};

/**
 * Describes the message loams.collection.v1.DeleteByFilterRequest.
 * Use `create(DeleteByFilterRequestSchema)` to create a new message.
 */
export const DeleteByFilterRequestSchema: GenMessage<DeleteByFilterRequest> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 14);

/**
 * @generated from message loams.collection.v1.PatchByFilterRequest
 */
export type PatchByFilterRequest = Message<"loams.collection.v1.PatchByFilterRequest"> & {
  /**
   * @generated from field: string namespace = 1;
   */
  namespace: string;

  /**
   * A collection name or an alias.
   *
   * @generated from field: string collection = 2;
   */
  collection: string;

  /**
   * @generated from field: google.protobuf.Value filter = 3;
   */
  filter?: Value | undefined;

  /**
   * What to change on each match, in the REST route's form. `id` and `upsert`
   * are refused: a filter write patches what the filter matched and never
   * creates a document.
   *
   * @generated from field: loams.collection.v1.Patch patch = 4;
   */
  patch?: Patch | undefined;

  /**
   * @generated from field: optional uint64 max_rows = 5;
   */
  maxRows?: bigint | undefined;

  /**
   * @generated from field: bool allow_partial = 6;
   */
  allowPartial: boolean;

  /**
   * @generated from field: loams.collection.v1.FilterWriteCursor cursor = 7;
   */
  cursor?: FilterWriteCursor | undefined;

  /**
   * @generated from field: loams.collection.v1.Consistency consistency = 8;
   */
  consistency?: Consistency | undefined;
};

/**
 * Describes the message loams.collection.v1.PatchByFilterRequest.
 * Use `create(PatchByFilterRequestSchema)` to create a new message.
 */
export const PatchByFilterRequestSchema: GenMessage<PatchByFilterRequest> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 15);

/**
 * The answer to one filter write. Both filter writes answer this, because
 * every number in it is the same question either way: how much the filter
 * matched, how much this call wrote, and the state it wrote at.
 *
 * @generated from message loams.collection.v1.FilterWriteResponse
 */
export type FilterWriteResponse = Message<"loams.collection.v1.FilterWriteResponse"> & {
  /**
   * Documents matching `filter` at the pin. The four counters are `optional`
   * and always set, for the reason `CountDocumentsResponse.count` gives: a
   * count of zero is a count, and proto3 JSON omitting it would read as
   * "unanswered".
   *
   * @generated from field: optional uint64 matched = 1;
   */
  matched?: bigint | undefined;

  /**
   * Documents this call deleted or patched.
   *
   * @generated from field: optional uint64 affected = 2;
   */
  affected?: bigint | undefined;

  /**
   * Keys this call's batches wrote: `affected`, and the keys found deleted
   * since the pin or (for a patch) already patched. It is what a caller's own
   * row budget counts.
   *
   * @generated from field: optional uint64 written = 3;
   */
  written?: bigint | undefined;

  /**
   * How many batches this call wrote.
   *
   * @generated from field: optional uint64 batches = 4;
   */
  batches?: bigint | undefined;

  /**
   * Whether the filter still matches rows this call did not reach.
   *
   * @generated from field: bool rows_remaining = 5;
   */
  rowsRemaining: boolean;

  /**
   * The cursor that finishes the write at the same pin. Present exactly when
   * `rows_remaining`.
   *
   * @generated from field: loams.collection.v1.FilterWriteCursor cursor = 6;
   */
  cursor?: FilterWriteCursor | undefined;

  /**
   * Covers the pin and every batch of this call, so a read at it sees this
   * write. The same value is the `loams-consistency-token` response header.
   *
   * @generated from field: string token = 7;
   */
  token: string;

  /**
   * The snapshot the filter was evaluated at, which the `cursor` resumes at.
   *
   * @generated from field: loams.collection.v1.Pin pin = 8;
   */
  pin?: Pin | undefined;

  /**
   * How long to wait, when the call stopped at its deadline on a batch refused
   * for backpressure. Absent when the call finished. The same number is the
   * `Retry-After` header of the call that carries it.
   *
   * @generated from field: optional uint64 retry_after_ms = 9;
   */
  retryAfterMs?: bigint | undefined;
};

/**
 * Describes the message loams.collection.v1.FilterWriteResponse.
 * Use `create(FilterWriteResponseSchema)` to create a new message.
 */
export const FilterWriteResponseSchema: GenMessage<FilterWriteResponse> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 16);

/**
 * @generated from message loams.collection.v1.FilterWriteCursor
 */
export type FilterWriteCursor = Message<"loams.collection.v1.FilterWriteCursor"> & {
  /**
   * The last key written; the next call starts after it.
   *
   * @generated from field: loams.collection.v1.DocumentId after = 1;
   */
  after?: DocumentId | undefined;

  /**
   * The snapshot the first call pinned, so the next call evaluates the same
   * filter against the same state.
   *
   * @generated from field: uint64 manifest_version = 2;
   */
  manifestVersion: bigint;

  /**
   * @generated from field: string token = 3;
   */
  token: string;
};

/**
 * Describes the message loams.collection.v1.FilterWriteCursor.
 * Use `create(FilterWriteCursorSchema)` to create a new message.
 */
export const FilterWriteCursorSchema: GenMessage<FilterWriteCursor> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 17);

/**
 * One document, as a get or a scroll answers it.
 *
 * @generated from message loams.collection.v1.Document
 */
export type Document = Message<"loams.collection.v1.Document"> & {
  /**
   * Absent only on the entry of an id that is not there.
   *
   * @generated from field: loams.collection.v1.DocumentId id = 1;
   */
  id?: DocumentId | undefined;

  /**
   * Absent when the projection leaves the source out, rather than an empty
   * object: "no source" and "an empty source" are different answers.
   *
   * @generated from field: google.protobuf.Struct source = 2;
   */
  source?: JsonObject | undefined;

  /**
   * Dense vectors by name, `map<string, google.protobuf.Value>` for the same
   * reason a `WriteDocument`'s are.
   *
   * @generated from field: map<string, google.protobuf.Value> vectors = 3;
   */
  vectors: { [key: string]: Value };

  /**
   * @generated from field: map<string, google.protobuf.Struct> sparse_vectors = 4;
   */
  sparseVectors: { [key: string]: JsonObject };

  /**
   * The typed field values by name, as the REST route's JSON (see the header).
   *
   * @generated from field: map<string, google.protobuf.Value> fields = 5;
   */
  fields: { [key: string]: Value };

  /**
   * Where the document's last write was appended. `optional` and always set
   * for the same reason as `OpPosition.seq_no`: the first write to a partition
   * is at 0, and "0" must not read as "no position".
   *
   * @generated from field: optional uint64 seq_no = 6;
   */
  seqNo?: bigint | undefined;

  /**
   * @generated from field: uint32 partition = 7;
   */
  partition: number;
};

/**
 * Describes the message loams.collection.v1.Document.
 * Use `create(DocumentSchema)` to create a new message.
 */
export const DocumentSchema: GenMessage<Document> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 18);

/**
 * How much of a read's freshness the caller asks for (rule 1).
 *
 * @generated from message loams.collection.v1.Consistency
 */
export type Consistency = Message<"loams.collection.v1.Consistency"> & {
  /**
   * Absent is `FRESHNESS_STRONG`: every write acknowledged before the read
   * began is visible. `FRESHNESS_EVENTUAL` is whatever the tail holds.
   *
   * @generated from field: loams.collection.v1.Freshness freshness = 1;
   */
  freshness: Freshness;

  /**
   * At least the writes this token names: the read **waits** for them and then
   * reads exactly the state up to them, which is what makes this a wait and
   * not a hint. A token this server cannot read is `invalid_argument`.
   *
   * @generated from field: string at_least = 2;
   */
  atLeast: string;

  /**
   * Exactly one retained manifest plus the token's writes after it, which is
   * what a filter write pins. A pinned read is not a free one.
   *
   * @generated from field: loams.collection.v1.Pin pin = 3;
   */
  pin?: Pin | undefined;
};

/**
 * Describes the message loams.collection.v1.Consistency.
 * Use `create(ConsistencySchema)` to create a new message.
 */
export const ConsistencySchema: GenMessage<Consistency> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 19);

/**
 * One pinned state of a collection: a manifest version and the token that
 * covers it. `Consistency.pin` names the state to read and
 * `FilterWriteResponse.pin` reports the one a filter write read.
 *
 * @generated from message loams.collection.v1.Pin
 */
export type Pin = Message<"loams.collection.v1.Pin"> & {
  /**
   * @generated from field: uint64 manifest_version = 1;
   */
  manifestVersion: bigint;

  /**
   * A `consistency.at_least` that reads this state again.
   *
   * @generated from field: string token = 2;
   */
  token: string;
};

/**
 * Describes the message loams.collection.v1.Pin.
 * Use `create(PinSchema)` to create a new message.
 */
export const PinSchema: GenMessage<Pin> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_document, 20);

/**
 * The outcome of one op (Ruling 10).
 *
 * @generated from enum loams.collection.v1.OpResult
 */
export enum OpResult {
  /**
   * @generated from enum value: OP_RESULT_UNSPECIFIED = 0;
   */
  UNSPECIFIED = 0,

  /**
   * The document was not there and now is.
   *
   * @generated from enum value: OP_RESULT_CREATED = 1;
   */
  CREATED = 1,

  /**
   * The document was there and now holds this write.
   *
   * @generated from enum value: OP_RESULT_UPDATED = 2;
   */
  UPDATED = 2,

  /**
   * The document was there and now is not.
   *
   * @generated from enum value: OP_RESULT_DELETED = 3;
   */
  DELETED = 3,

  /**
   * There was no such document (only reported with `report_existence`).
   *
   * @generated from enum value: OP_RESULT_NOT_FOUND = 4;
   */
  NOT_FOUND = 4,

  /**
   * A patch whose result equals the document that is already there.
   *
   * @generated from enum value: OP_RESULT_NOOP = 5;
   */
  NOOP = 5,

  /**
   * Written, without existence reporting.
   *
   * @generated from enum value: OP_RESULT_ACCEPTED = 6;
   */
  ACCEPTED = 6,
}

/**
 * Describes the enum loams.collection.v1.OpResult.
 */
export const OpResultSchema: GenEnum<OpResult> = /*@__PURE__*/
  enumDesc(file_loams_collection_v1_document, 0);

/**
 * @generated from enum loams.collection.v1.Freshness
 */
export enum Freshness {
  /**
   * @generated from enum value: FRESHNESS_UNSPECIFIED = 0;
   */
  UNSPECIFIED = 0,

  /**
   * Every write acknowledged before the read began.
   *
   * @generated from enum value: FRESHNESS_STRONG = 1;
   */
  STRONG = 1,

  /**
   * Whatever the tail holds right now.
   *
   * @generated from enum value: FRESHNESS_EVENTUAL = 2;
   */
  EVENTUAL = 2,
}

/**
 * Describes the enum loams.collection.v1.Freshness.
 */
export const FreshnessSchema: GenEnum<Freshness> = /*@__PURE__*/
  enumDesc(file_loams_collection_v1_document, 1);

/**
 * @generated from service loams.collection.v1.DocumentService
 */
export const DocumentService: GenService<{
  /**
   * An atomic write: every op of a request is applied or none is (Ruling 16).
   *
   * This is the one RPC here that is not naturally idempotent, so it carries
   * `idempotency_key`; a repeat of a keyed write replays the first answer
   * instead of writing again. The write is refused with `resource_exhausted`
   * when it would take the collection's unapplied-data budget over its
   * ceiling, and every answer to an admitted *or* a refused write carries the
   * backlog headers.
   *
   * @generated from rpc loams.collection.v1.DocumentService.WriteDocuments
   */
  writeDocuments: {
    methodKind: "unary";
    input: typeof WriteDocumentsRequestSchema;
    output: typeof WriteDocumentsResponseSchema;
  },
  /**
   * The documents of `ids`, in request order.
   *
   * A document that is not there is an **entry with no fields**, which sits at
   * the requested id's position: proto3 JSON has no `null` for an element of a
   * `repeated` field of messages, and an empty entry is the same information
   * — the entry is there and it is empty.
   *
   * @generated from rpc loams.collection.v1.DocumentService.GetDocuments
   */
  getDocuments: {
    methodKind: "unary";
    input: typeof GetDocumentsRequestSchema;
    output: typeof GetDocumentsResponseSchema;
  },
  /**
   * One page of the collection's documents in primary-key order, and the id to
   * continue after. `next` is absent on the last page.
   *
   * @generated from rpc loams.collection.v1.DocumentService.ScrollDocuments
   */
  scrollDocuments: {
    methodKind: "unary";
    input: typeof ScrollDocumentsRequestSchema;
    output: typeof ScrollDocumentsResponseSchema;
  },
  /**
   * How many documents match, which is a read and not a free one.
   *
   * @generated from rpc loams.collection.v1.DocumentService.CountDocuments
   */
  countDocuments: {
    methodKind: "unary";
    input: typeof CountDocumentsRequestSchema;
    output: typeof CountDocumentsResponseSchema;
  },
  /**
   * Deletes every document matching `filter`, in batches, at one pinned
   * snapshot (M1.5 Task 9a, D87).
   *
   * A call matching more than `max_rows` fails with `invalid_argument` and
   * deletes nothing unless `allow_partial`, in which case it writes what fits
   * and answers the `cursor` that finishes it at the same pin. The pin travels
   * in `consistency` too: an `at_least` token makes the pin at least as new as
   * the token.
   *
   * @generated from rpc loams.collection.v1.DocumentService.DeleteByFilter
   */
  deleteByFilter: {
    methodKind: "unary";
    input: typeof DeleteByFilterRequestSchema;
    output: typeof FilterWriteResponseSchema;
  },
  /**
   * The same as `DeleteByFilter` with a `patch` applied to each match.
   *
   * A filter write patches the documents the filter matched and never creates
   * one, so `patch.upsert` is refused.
   *
   * @generated from rpc loams.collection.v1.DocumentService.PatchByFilter
   */
  patchByFilter: {
    methodKind: "unary";
    input: typeof PatchByFilterRequestSchema;
    output: typeof FilterWriteResponseSchema;
  },
}> = /*@__PURE__*/
  serviceDesc(file_loams_collection_v1_document, 0);

