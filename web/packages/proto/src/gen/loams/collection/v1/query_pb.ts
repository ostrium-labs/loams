// Queries: the search IR as typed messages, and its answer (design §44 §5.1,
// D602; API1 Task 4).
//
// This is the third file of `loams.collection.v1`. It replaces the M1.2
// native REST route `POST /v1/namespaces/{ns}/query` — the `SearchRequest` of
// `crates/loams-query/src/ir.rs`, which is the IR the REST route parses
// verbatim — with `QueryService/Search` on the one port (design §44 §4).
//
// - **Names are fields, not path segments.** `namespace` and `collection`
//   travel in the request message and one path serves every collection (plan
//   ruling 4), exactly as `collection.proto` and `document.proto` do.
// - **proto3 JSON is the wire form**: field names are `lowerCamelCase`, an
//   enum answers its proto name in `UPPER_SNAKE`, a 64-bit integer answers a
//   decimal string, and a field at its default is *absent* rather than `0`,
//   `false`, `""` or `[]`.
//
// ## The one divergence that is unavoidable: `snake_case` → `lowerCamelCase`
//
// The REST IR is serde `snake_case` and proto3 JSON is `lowerCamelCase`, so
// `score_threshold` is `scoreThreshold`, `minimum_should_match` is
// `minimumShouldMatch`, `track_total_hits` is `trackTotalHits` and
// `must_not` is `mustNot`. Nothing here *renames* anything else: the IR's field
// names are the proto field names, so the two surfaces are one vocabulary with
// two spellings and no translation table to keep.
//
// Three rules follow from proto3 JSON rather than from the IR, and
// `crates/loams/tests/connect_query.rs` pins all three:
//
// - **A `null` is an absent field.** proto3 JSON has no `null` for a scalar or
//   a message field, and a field at its default is *absent*. The REST fixture
//   spells its defaults out (`"nprobes": null`, `"fusion": null`), and on this
//   surface the same value is the field being absent. That is why every
//   optional IR field is `optional` or a message rather than a sentinel: a
//   caller that must distinguish "not asked" from "asked for zero" needs the
//   presence, and proto3 gives it for `optional`.
// - **A oneof arm is `{"arm": {…}}`, and a unit arm is `{"arm": {}}`.** The IR
//   spells `Query::MatchAll` as the bare string `"match_all"`, which a proto
//   oneof has no spelling for, so `match_all` is an empty message. `WriteOp`
//   set this precedent in this package, and `Query`, `Retriever`, `Fusion`,
//   `SortKey` and `TrackTotalHits` follow it.
// - **A bare id is a `DocumentId`.** `Query::Ids` is a `repeated DocumentId`
//   and `Hit.pk` is one, for Task 3's reason: `google.protobuf.Value` holds
//   every number in a `double`, so a `u64::MAX` id would come back as
//   `18446744073709552000` — a different document.
//
// ## The §05 §4 body is a spelling of this request, not another request
//
// Design §05 §4's example body is accepted by the same RPC, and this file
// declares the four keys that reach it verbatim rather than through the typed
// messages:
//
// | §05 §4 | here |
// |---|---|
// | `"from": "collections.kb"` | `SearchRequest.from`, an alias of `collection` |
// | `"retrieve": [...]` | `SearchRequest.retrieve`, an alias of `retrievers` |
// | `"fuse": {"method": "rrf", "k": 60}` | `SearchRequest.fuse`, a `Struct`, verbatim |
// | `"select": ["id", "_score", "body"]` | `SearchRequest.select`, the projection `Struct` of Task 2/3 |
//
// `fuse` is a `Struct` rather than a `Fusion` on purpose. §05 §4 names its
// method with the string `"rrf"`, and ruling 2.1's shape for this package is
// the IR spelling rather than a new `FUSION_METHOD_*` enumeration, so the
// `Struct` keeps `method` as the string a reader already switches on
// verbatim — the same reason `Patch.mode` and `ScanColumn.role` are strings.
// A field-for-field verbatim decode of the whole §05 §4 body is impossible
// (a message field has no bare-string form), so these four keys are the
// minimal reading that keeps the plan's sentence true: `from` and `retrieve`
// are the field aliases, `fuse` and `select` are the two payloads the IR
// spells differently. Everything else of §05 §4 — `consistency`, the
// retrievers, `limit`, `offset` — is this message's own spelling.
//
// `from` and `retrieve` are *aliases*, not a second request: they are read
// into the same fields `collection` and `retrievers` hold, and a request that
// sets both is not two requests. `collection` wins over `from` and
// `retrievers` over `retrieve`, because the typed field is the one a generated
// SDK fills in and the alias exists for a caller migrating a §05 §4 body.
//
// ## `rerank` and `expand` are declared so they can be refused
//
// proto3 JSON says to skip a field the message does not declare. If `rerank`
// and `expand` were simply absent, a body carrying either would be accepted
// with the stage silently dropped — the one failure a caller cannot detect.
// They are therefore declared as `Struct`s that **every implementation
// refuses**: both stages need graph expansion (design §07) and arrive in M3,
// and a caller that sends one gets `invalid_argument` saying so, on both
// surfaces, from the same code that refuses it over REST. This is the only
// reason they are here, and the two `Struct`s exist to be rejected rather than
// to carry a payload.
//
// ## `track_total_hits` is a `Value`, and the reason is the REST spelling
//
// Every other field of this message is typed, and this one is not: the IR's
// `TrackTotalHits` has two spellings a caller may send — the serde
// externally-tagged bare string `"exact"` and the proto3 JSON message form
// `{"exact": {}}` — and a proto3 message field has only the second of them.
// Carrying the union as a `google.protobuf.Value` is what lets a body written
// against the REST IR and one written against this proto both mean "count
// exactly", instead of forcing every such caller to learn a second spelling.
// The handler maps the `Value` to the IR by its own rules
// (`crates/loams/src/api/connect_query_ir.rs`) and refuses anything else.
//
// ## `filter` here is the typed IR; on the document RPCs it is still a `Value`
//
// `ScrollDocumentsRequest.filter` and its three siblings stay
// `google.protobuf.Value`, and `document.proto` says why: Task 3 shipped them
// that way and eighteen green tests send the bare string `"match_all"`, which a
// typed oneof cannot spell. `SearchRequest.filter` is the typed [`Query`], so
// this is the one filter on the package a caller cannot degrade by sending a
// string the handler did not read.
//
// ## What stays a `Struct` or a `Value` here, and why
//
// The IR's open parts stay open, for `document.proto`'s reasons:
// `SearchRequest.select` (a projection's `source` has a JSON object form),
// `SearchRequest.fuse` (above), `SparseQuery.query` (a sparse vector is
// `{"indices": […], "values": […]}`) and `SearchRequest.aggregations` (an
// ES aggregation request, which Tantivy's own `Aggregations` owns). A field
// value, a sort value and a hit's `highlight`/`fields` entry are `Value`s
// because the IR's own JSON for one is a scalar or `{"date": "…"}` /
// `{"uuid": "…"}` (Ruling 9), and all three spellings fit a `Value`.
//
// ## Why `ScrollDocuments` stays unary
//
// Plan ruling 2 offers `page_token` as an *addition* to the shipped unary
// `DocumentService/ScrollDocuments`, and Q614 (server-streaming versus unary
// pagination) is unanswered. This file therefore does not change that RPC's
// streaming-ness — eighteen shipped tests would break for an open question —
// and `document.proto` gains `ScrollDocumentsRequest.page_token`, the
// AIP-158 spelling of the **same** `DocumentId` cursor `after` already is.
// True server-streaming, if Q614 says so, is a separate additive RPC.
//
// ## The handlers are thin
//
// `Search` calls the same `CollectionService::search` the REST route calls and
// nothing else: the request message is turned back into the native REST JSON
// and `loams_query::json::hybrid::parse_query_body` — the REST route's own
// parser — reads it, so a filter, a retriever, a fusion or a sort key cannot
// behave differently on the two surfaces. That is what
// `crates/loams/tests/connect_query.rs::routing_and_ranking_match_rest_for_every_fixture`
// checks, fifteen fixtures compared against each other rather than against a
// golden number.
//
// ## The SDK module, and why no method is in the SDK facade yet
//
// The service carries `loams.options.v1.ModuleOptions` (`loams.search`), which
// is what §44 §7.3 asks of every public package; §7.2 puts `loams.search` and
// `loams.vector` on this one RPC.
//
// No method carries `FacadeOptions`, which is plan ruling 2.5's shape again: a
// `facade` option makes `protoc-gen-loams-facade` emit a call, and the call
// imports the package's message types from the generated stubs, which means
// every SDK mirror has to generate `loams.collection.v1` first. That is SDK1's
// stub task, and Task 2 measured what going ahead of it costs.

// @generated by protoc-gen-es v2.16.0 with parameter "target=ts,import_extension=js"
// @generated from file loams/collection/v1/query.proto (package loams.collection.v1, syntax proto3)
/* eslint-disable */

import type { GenEnum, GenFile, GenMessage, GenService } from "@bufbuild/protobuf/codegenv2";
import { enumDesc, fileDesc, messageDesc, serviceDesc } from "@bufbuild/protobuf/codegenv2";
import type { Empty, Value } from "@bufbuild/protobuf/wkt";
import { file_google_protobuf_empty, file_google_protobuf_struct } from "@bufbuild/protobuf/wkt";
import type { Consistency, DocumentId } from "./document_pb.js";
import { file_loams_collection_v1_document } from "./document_pb.js";
import { file_loams_options_v1_options } from "../../options/v1/options_pb.js";
import type { JsonObject, Message } from "@bufbuild/protobuf";

/**
 * Describes the file loams/collection/v1/query.proto.
 */
export const file_loams_collection_v1_query: GenFile = /*@__PURE__*/
  fileDesc("Ch9sb2Ftcy9jb2xsZWN0aW9uL3YxL3F1ZXJ5LnByb3RvEhNsb2Ftcy5jb2xsZWN0aW9uLnYxItkGCg1TZWFyY2hSZXF1ZXN0EhEKCW5hbWVzcGFjZRgBIAEoCRISCgpjb2xsZWN0aW9uGAIgASgJEjUKC2NvbnNpc3RlbmN5GAMgASgLMiAubG9hbXMuY29sbGVjdGlvbi52MS5Db25zaXN0ZW5jeRIyCgpyZXRyaWV2ZXJzGAQgAygLMh4ubG9hbXMuY29sbGVjdGlvbi52MS5SZXRyaWV2ZXISKwoGZnVzaW9uGAUgASgLMhsubG9hbXMuY29sbGVjdGlvbi52MS5GdXNpb24SKgoGZmlsdGVyGAYgASgLMhoubG9hbXMuY29sbGVjdGlvbi52MS5RdWVyeRIqCgRzb3J0GAcgAygLMhwubG9hbXMuY29sbGVjdGlvbi52MS5Tb3J0S2V5Eg4KBm9mZnNldBgIIAEoDRISCgVsaW1pdBgJIAEoDUgAiAEBEiwKDHNlYXJjaF9hZnRlchgKIAMoCzIWLmdvb2dsZS5wcm90b2J1Zi5WYWx1ZRIcCg9zY29yZV90aHJlc2hvbGQYCyABKAJIAYgBARInCgZzZWxlY3QYDCABKAsyFy5nb29nbGUucHJvdG9idWYuU3RydWN0EiwKDGFnZ3JlZ2F0aW9ucxgNIAEoCzIWLmdvb2dsZS5wcm90b2J1Zi5WYWx1ZRIxCgloaWdobGlnaHQYDiABKAsyHi5sb2Ftcy5jb2xsZWN0aW9uLnYxLkhpZ2hsaWdodBIuCghncm91cF9ieRgPIAEoCzIcLmxvYW1zLmNvbGxlY3Rpb24udjEuR3JvdXBCeRIwChB0cmFja190b3RhbF9oaXRzGBAgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlEicKBnJlcmFuaxgRIAEoCzIXLmdvb2dsZS5wcm90b2J1Zi5TdHJ1Y3QSJwoGZXhwYW5kGBIgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdBIMCgRmcm9tGBMgASgJEjAKCHJldHJpZXZlGBQgAygLMh4ubG9hbXMuY29sbGVjdGlvbi52MS5SZXRyaWV2ZXISJQoEZnVzZRgVIAEoCzIXLmdvb2dsZS5wcm90b2J1Zi5TdHJ1Y3RCCAoGX2xpbWl0QhIKEF9zY29yZV90aHJlc2hvbGQimAIKCVJldHJpZXZlchIyCgZ2ZWN0b3IYASABKAsyIC5sb2Ftcy5jb2xsZWN0aW9uLnYxLlZlY3RvclF1ZXJ5SAASLgoEdGV4dBgCIAEoCzIeLmxvYW1zLmNvbGxlY3Rpb24udjEuVGV4dFF1ZXJ5SAASMAoFZnVzZWQYAyABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLkZ1c2VkUXVlcnlIABI0CgdyZXNjb3JlGAQgASgLMiEubG9hbXMuY29sbGVjdGlvbi52MS5SZXNjb3JlUXVlcnlIABIyCgZzcGFyc2UYBSABKAsyIC5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNwYXJzZVF1ZXJ5SABCCwoJcmV0cmlldmVyIpIBCgtWZWN0b3JRdWVyeRINCgVmaWVsZBgBIAEoCRINCgVxdWVyeRgCIAMoAhIJCgFrGAMgASgNEi4KBnBhcmFtcxgEIAEoCzIeLmxvYW1zLmNvbGxlY3Rpb24udjEuQW5uUGFyYW1zEioKBmZpbHRlchgFIAEoCzIaLmxvYW1zLmNvbGxlY3Rpb24udjEuUXVlcnkiQQoJVGV4dFF1ZXJ5EikKBXF1ZXJ5GAEgASgLMhoubG9hbXMuY29sbGVjdGlvbi52MS5RdWVyeRIJCgFrGAIgASgNInQKCkZ1c2VkUXVlcnkSLgoGaW5wdXRzGAEgAygLMh4ubG9hbXMuY29sbGVjdGlvbi52MS5SZXRyaWV2ZXISKwoGZnVzaW9uGAIgASgLMhsubG9hbXMuY29sbGVjdGlvbi52MS5GdXNpb24SCQoBaxgDIAEoDSJmCgxSZXNjb3JlUXVlcnkSLQoFaW5wdXQYASABKAsyHi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlJldHJpZXZlchINCgVmaWVsZBgCIAEoCRINCgVxdWVyeRgDIAMoAhIJCgFrGAQgASgNIq4BCgtTcGFyc2VRdWVyeRINCgVmaWVsZBgBIAEoCRImCgVxdWVyeRgCIAEoCzIXLmdvb2dsZS5wcm90b2J1Zi5TdHJ1Y3QSCQoBaxgDIAEoDRIqCgZmaWx0ZXIYBCABKAsyGi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlF1ZXJ5EjEKBnBhcmFtcxgFIAEoCzIhLmxvYW1zLmNvbGxlY3Rpb24udjEuU3BhcnNlUGFyYW1zIsABCglBbm5QYXJhbXMSDQoFZXhhY3QYASABKAgSFAoHbnByb2JlcxgCIAEoDUgAiAEBEhoKDXJlZmluZV9mYWN0b3IYAyABKA1IAYgBARIPCgJlZhgEIAEoDUgCiAEBEhkKDG92ZXJzYW1wbGluZxgFIAEoAkgDiAEBEhAKCGRpc3RhbmNlGAYgASgJQgoKCF9ucHJvYmVzQhAKDl9yZWZpbmVfZmFjdG9yQgUKA19lZkIPCg1fb3ZlcnNhbXBsaW5nIj4KDFNwYXJzZVBhcmFtcxIuCgppZGZfY29ycHVzGAEgASgLMhoubG9hbXMuY29sbGVjdGlvbi52MS5RdWVyeSKdAQoGRnVzaW9uEicKA3JyZhgBIAEoCzIYLmxvYW1zLmNvbGxlY3Rpb24udjEuUnJmSAASJgoEZGJzZhgCIAEoCzIWLmdvb2dsZS5wcm90b2J1Zi5FbXB0eUgAEjgKDHdlaWdodGVkX3N1bRgDIAEoCzIgLmxvYW1zLmNvbGxlY3Rpb24udjEuV2VpZ2h0ZWRTdW1IAEIICgZmdXNpb24iGwoDUnJmEg4KAWsYASABKA1IAIgBAUIECgJfayIeCgtXZWlnaHRlZFN1bRIPCgd3ZWlnaHRzGAEgAygCIq8ICgVRdWVyeRIrCgltYXRjaF9hbGwYASABKAsyFi5nb29nbGUucHJvdG9idWYuRW1wdHlIABIsCgptYXRjaF9ub25lGAIgASgLMhYuZ29vZ2xlLnByb3RvYnVmLkVtcHR5SAASMAoFbWF0Y2gYAyABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLk1hdGNoUXVlcnlIABI9CgxtYXRjaF9waHJhc2UYBCABKAsyJS5sb2Ftcy5jb2xsZWN0aW9uLnYxLk1hdGNoUGhyYXNlUXVlcnlIABI7CgttdWx0aV9tYXRjaBgFIAEoCzIkLmxvYW1zLmNvbGxlY3Rpb24udjEuTXVsdGlNYXRjaFF1ZXJ5SAASLgoEdGVybRgGIAEoCzIeLmxvYW1zLmNvbGxlY3Rpb24udjEuVGVybVF1ZXJ5SAASMAoFdGVybXMYByABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLlRlcm1zUXVlcnlIABIwCgVyYW5nZRgIIAEoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuUmFuZ2VRdWVyeUgAEjEKBmV4aXN0cxgJIAEoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuRmllbGRRdWVyeUgAEjIKB2lzX251bGwYCiABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLkZpZWxkUXVlcnlIABIzCghpc19lbXB0eRgLIAEoCzIfLmxvYW1zLmNvbGxlY3Rpb24udjEuRmllbGRRdWVyeUgAEj0KDHZhbHVlc19jb3VudBgMIAEoCzIlLmxvYW1zLmNvbGxlY3Rpb24udjEuVmFsdWVzQ291bnRRdWVyeUgAEjIKBnByZWZpeBgNIAEoCzIgLmxvYW1zLmNvbGxlY3Rpb24udjEuUHJlZml4UXVlcnlIABI2Cgh3aWxkY2FyZBgOIAEoCzIiLmxvYW1zLmNvbGxlY3Rpb24udjEuV2lsZGNhcmRRdWVyeUgAEjAKBWZ1enp5GA8gASgLMh8ubG9hbXMuY29sbGVjdGlvbi52MS5GdXp6eVF1ZXJ5SAASPQoMcXVlcnlfc3RyaW5nGBEgASgLMiUubG9hbXMuY29sbGVjdGlvbi52MS5RdWVyeVN0cmluZ1F1ZXJ5SAASLgoEYm9vbBgSIAEoCzIeLmxvYW1zLmNvbGxlY3Rpb24udjEuQm9vbFF1ZXJ5SAASMAoFYm9vc3QYEyABKAsyHy5sb2Ftcy5jb2xsZWN0aW9uLnYxLkJvb3N0UXVlcnlIABJBCg5jb25zdGFudF9zY29yZRgUIAEoCzInLmxvYW1zLmNvbGxlY3Rpb24udjEuQ29uc3RhbnRTY29yZVF1ZXJ5SAASIwoDaWRzGBAgAygLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlQgcKBXF1ZXJ5IrwBCgpNYXRjaFF1ZXJ5Eg0KBWZpZWxkGAEgASgJEgwKBHRleHQYAiABKAkSEAoIb3BlcmF0b3IYAyABKAkSIQoUbWluaW11bV9zaG91bGRfbWF0Y2gYBCABKAlIAIgBARIxCglmdXp6aW5lc3MYBSABKAsyHi5sb2Ftcy5jb2xsZWN0aW9uLnYxLkZ1enppbmVzcxIQCghhbmFseXplchgGIAEoCUIXChVfbWluaW11bV9zaG91bGRfbWF0Y2giPQoQTWF0Y2hQaHJhc2VRdWVyeRINCgVmaWVsZBgBIAEoCRIMCgR0ZXh0GAIgASgJEgwKBHNsb3AYAyABKA0i5wEKD011bHRpTWF0Y2hRdWVyeRI0CgZmaWVsZHMYASADKAsyJC5sb2Ftcy5jb2xsZWN0aW9uLnYxLk11bHRpTWF0Y2hGaWVsZBIMCgR0ZXh0GAIgASgJEjEKBGtpbmQYAyABKA4yIy5sb2Ftcy5jb2xsZWN0aW9uLnYxLk11bHRpTWF0Y2hLaW5kEjMKCG9wZXJhdG9yGAQgASgOMiEubG9hbXMuY29sbGVjdGlvbi52MS5Cb29sT3BlcmF0b3ISGAoLdGllX2JyZWFrZXIYBSABKAJIAIgBAUIOCgxfdGllX2JyZWFrZXIiLwoPTXVsdGlNYXRjaEZpZWxkEg0KBWZpZWxkGAEgASgJEg0KBWJvb3N0GAIgASgCIkEKCVRlcm1RdWVyeRINCgVmaWVsZBgBIAEoCRIlCgV2YWx1ZRgCIAEoCzIWLmdvb2dsZS5wcm90b2J1Zi5WYWx1ZSJDCgpUZXJtc1F1ZXJ5Eg0KBWZpZWxkGAEgASgJEiYKBnZhbHVlcxgCIAMoCzIWLmdvb2dsZS5wcm90b2J1Zi5WYWx1ZSKtAQoKUmFuZ2VRdWVyeRINCgVmaWVsZBgBIAEoCRIiCgJndBgCIAEoCzIWLmdvb2dsZS5wcm90b2J1Zi5WYWx1ZRIjCgNndGUYAyABKAsyFi5nb29nbGUucHJvdG9idWYuVmFsdWUSIgoCbHQYBCABKAsyFi5nb29nbGUucHJvdG9idWYuVmFsdWUSIwoDbHRlGAUgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlIhsKCkZpZWxkUXVlcnkSDQoFZmllbGQYASABKAkihQEKEFZhbHVlc0NvdW50UXVlcnkSDQoFZmllbGQYASABKAkSDwoCZ3QYAiABKARIAIgBARIQCgNndGUYAyABKARIAYgBARIPCgJsdBgEIAEoBEgCiAEBEhAKA2x0ZRgFIAEoBEgDiAEBQgUKA19ndEIGCgRfZ3RlQgUKA19sdEIGCgRfbHRlIisKC1ByZWZpeFF1ZXJ5Eg0KBWZpZWxkGAEgASgJEg0KBXZhbHVlGAIgASgJIi8KDVdpbGRjYXJkUXVlcnkSDQoFZmllbGQYASABKAkSDwoHcGF0dGVybhgCIAEoCSJdCgpGdXp6eVF1ZXJ5Eg0KBWZpZWxkGAEgASgJEg0KBXZhbHVlGAIgASgJEjEKCWZ1enppbmVzcxgDIAEoCzIeLmxvYW1zLmNvbGxlY3Rpb24udjEuRnV6emluZXNzInYKEFF1ZXJ5U3RyaW5nUXVlcnkSDQoFcXVlcnkYASABKAkSFgoOZGVmYXVsdF9maWVsZHMYAiADKAkSOwoQZGVmYXVsdF9vcGVyYXRvchgDIAEoDjIhLmxvYW1zLmNvbGxlY3Rpb24udjEuQm9vbE9wZXJhdG9yIvcBCglCb29sUXVlcnkSKAoEbXVzdBgBIAMoCzIaLmxvYW1zLmNvbGxlY3Rpb24udjEuUXVlcnkSKgoGc2hvdWxkGAIgAygLMhoubG9hbXMuY29sbGVjdGlvbi52MS5RdWVyeRIsCghtdXN0X25vdBgDIAMoCzIaLmxvYW1zLmNvbGxlY3Rpb24udjEuUXVlcnkSKgoGZmlsdGVyGAQgAygLMhoubG9hbXMuY29sbGVjdGlvbi52MS5RdWVyeRIhChRtaW5pbXVtX3Nob3VsZF9tYXRjaBgFIAEoCUgAiAEBQhcKFV9taW5pbXVtX3Nob3VsZF9tYXRjaCJGCgpCb29zdFF1ZXJ5EikKBXF1ZXJ5GAEgASgLMhoubG9hbXMuY29sbGVjdGlvbi52MS5RdWVyeRINCgVib29zdBgCIAEoAiJOChJDb25zdGFudFNjb3JlUXVlcnkSKQoFcXVlcnkYASABKAsyGi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlF1ZXJ5Eg0KBXNjb3JlGAIgASgCIlEKCUZ1enppbmVzcxImCgRhdXRvGAEgASgLMhYuZ29vZ2xlLnByb3RvYnVmLkVtcHR5SAASDwoFZWRpdHMYAiABKA1IAEILCglmdXp6aW5lc3MipQEKB1NvcnRLZXkSLwoFc2NvcmUYASABKAsyHi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNjb3JlU29ydEgAEjEKAnBrGAIgASgLMiMubG9hbXMuY29sbGVjdGlvbi52MS5QcmltYXJ5S2V5U29ydEgAEi8KBWZpZWxkGAMgASgLMh4ubG9hbXMuY29sbGVjdGlvbi52MS5GaWVsZFNvcnRIAEIFCgNrZXkiOgoJU2NvcmVTb3J0Ei0KBW9yZGVyGAEgASgOMh4ubG9hbXMuY29sbGVjdGlvbi52MS5Tb3J0T3JkZXIiPwoOUHJpbWFyeUtleVNvcnQSLQoFb3JkZXIYASABKA4yHi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNvcnRPcmRlciJ9CglGaWVsZFNvcnQSDQoFZmllbGQYASABKAkSLQoFb3JkZXIYAiABKA4yHi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNvcnRPcmRlchIyCgdtaXNzaW5nGAMgASgOMiEubG9hbXMuY29sbGVjdGlvbi52MS5NaXNzaW5nT3JkZXIiQAoJSGlnaGxpZ2h0EjMKBmZpZWxkcxgBIAMoCzIjLmxvYW1zLmNvbGxlY3Rpb24udjEuSGlnaGxpZ2h0RmllbGQizQEKDkhpZ2hsaWdodEZpZWxkEg0KBWZpZWxkGAEgASgJEhQKB3ByZV90YWcYAiABKAlIAIgBARIVCghwb3N0X3RhZxgDIAEoCUgBiAEBEhoKDWZyYWdtZW50X3NpemUYBCABKA1IAogBARIgChNudW1iZXJfb2ZfZnJhZ21lbnRzGAUgASgNSAOIAQFCCgoIX3ByZV90YWdCCwoJX3Bvc3RfdGFnQhAKDl9mcmFnbWVudF9zaXplQhYKFF9udW1iZXJfb2ZfZnJhZ21lbnRzIl4KB0dyb3VwQnkSDQoFZmllbGQYASABKAkSFwoKZ3JvdXBfc2l6ZRgCIAEoDUgAiAEBEhIKBWxpbWl0GAMgASgNSAGIAQFCDQoLX2dyb3VwX3NpemVCCAoGX2xpbWl0IpgCCg5TZWFyY2hSZXNwb25zZRImCgRoaXRzGAEgAygLMhgubG9hbXMuY29sbGVjdGlvbi52MS5IaXQSLQoFdG90YWwYAiABKAsyHi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlRvdGFsSGl0cxIsCgxhZ2dyZWdhdGlvbnMYAyABKAsyFi5nb29nbGUucHJvdG9idWYuVmFsdWUSLQoGZ3JvdXBzGAQgAygLMh0ubG9hbXMuY29sbGVjdGlvbi52MS5IaXRHcm91cBISCgpyZWFkX3Rva2VuGAUgASgJEhAKCGhvdF91c2VkGAYgAygJEiwKC3BlcmZvcm1hbmNlGAcgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdCKuBQoDSGl0EisKAnBrGAEgASgLMh8ubG9hbXMuY29sbGVjdGlvbi52MS5Eb2N1bWVudElkEg0KBXNjb3JlGAIgASgCEisKC3NvcnRfdmFsdWVzGAMgAygLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlEicKBnNvdXJjZRgEIAEoCzIXLmdvb2dsZS5wcm90b2J1Zi5TdHJ1Y3QSNgoHdmVjdG9ycxgFIAMoCzIlLmxvYW1zLmNvbGxlY3Rpb24udjEuSGl0LlZlY3RvcnNFbnRyeRJDCg5zcGFyc2VfdmVjdG9ycxgGIAMoCzIrLmxvYW1zLmNvbGxlY3Rpb24udjEuSGl0LlNwYXJzZVZlY3RvcnNFbnRyeRI6CgloaWdobGlnaHQYByADKAsyJy5sb2Ftcy5jb2xsZWN0aW9uLnYxLkhpdC5IaWdobGlnaHRFbnRyeRI0CgZmaWVsZHMYCCADKAsyJC5sb2Ftcy5jb2xsZWN0aW9uLnYxLkhpdC5GaWVsZHNFbnRyeRpGCgxWZWN0b3JzRW50cnkSCwoDa2V5GAEgASgJEiUKBXZhbHVlGAIgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlOgI4ARpNChJTcGFyc2VWZWN0b3JzRW50cnkSCwoDa2V5GAEgASgJEiYKBXZhbHVlGAIgASgLMhcuZ29vZ2xlLnByb3RvYnVmLlN0cnVjdDoCOAEaSAoOSGlnaGxpZ2h0RW50cnkSCwoDa2V5GAEgASgJEiUKBXZhbHVlGAIgASgLMhYuZ29vZ2xlLnByb3RvYnVmLlZhbHVlOgI4ARpFCgtGaWVsZHNFbnRyeRILCgNrZXkYASABKAkSJQoFdmFsdWUYAiABKAsyFi5nb29nbGUucHJvdG9idWYuVmFsdWU6AjgBIlcKCEhpdEdyb3VwEiMKA2tleRgBIAEoCzIWLmdvb2dsZS5wcm90b2J1Zi5WYWx1ZRImCgRoaXRzGAIgAygLMhgubG9hbXMuY29sbGVjdGlvbi52MS5IaXQiXwoJVG90YWxIaXRzEhIKBXZhbHVlGAEgASgESACIAQESNAoIcmVsYXRpb24YAiABKA4yIi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlRvdGFsUmVsYXRpb25CCAoGX3ZhbHVlKloKDEJvb2xPcGVyYXRvchIdChlCT09MX09QRVJBVE9SX1VOU1BFQ0lGSUVEEAASFAoQQk9PTF9PUEVSQVRPUl9PUhABEhUKEUJPT0xfT1BFUkFUT1JfQU5EEAIq2gEKDk11bHRpTWF0Y2hLaW5kEiAKHE1VTFRJX01BVENIX0tJTkRfVU5TUEVDSUZJRUQQABIgChxNVUxUSV9NQVRDSF9LSU5EX0JFU1RfRklFTERTEAESIAocTVVMVElfTUFUQ0hfS0lORF9NT1NUX0ZJRUxEUxACEiEKHU1VTFRJX01BVENIX0tJTkRfQ1JPU1NfRklFTERTEAMSGwoXTVVMVElfTUFUQ0hfS0lORF9QSFJBU0UQBBIiCh5NVUxUSV9NQVRDSF9LSU5EX1BIUkFTRV9QUkVGSVgQBSpQCglTb3J0T3JkZXISGgoWU09SVF9PUkRFUl9VTlNQRUNJRklFRBAAEhIKDlNPUlRfT1JERVJfQVNDEAESEwoPU09SVF9PUkRFUl9ERVNDEAIqXgoMTWlzc2luZ09yZGVyEh0KGU1JU1NJTkdfT1JERVJfVU5TUEVDSUZJRUQQABIXChNNSVNTSU5HX09SREVSX0ZJUlNUEAESFgoSTUlTU0lOR19PUkRFUl9MQVNUEAIqXgoNVG90YWxSZWxhdGlvbhIeChpUT1RBTF9SRUxBVElPTl9VTlNQRUNJRklFRBAAEhUKEVRPVEFMX1JFTEFUSU9OX0VREAESFgoSVE9UQUxfUkVMQVRJT05fR1RFEAIyvQEKDFF1ZXJ5U2VydmljZRJWCgZTZWFyY2gSIi5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNlYXJjaFJlcXVlc3QaIy5sb2Ftcy5jb2xsZWN0aW9uLnYxLlNlYXJjaFJlc3BvbnNlIgOQAgEaVYq1GFEKBnNlYXJjaBJHUXVlcmllczogb25lIHNlYXJjaCBvdmVyIG9uZSBjb2xsZWN0aW9uLCBkZW5zZSwgc3BhcnNlLCB0ZXh0IG9yIGh5YnJpZC5iBnByb3RvMw", [file_google_protobuf_empty, file_google_protobuf_struct, file_loams_collection_v1_document, file_loams_options_v1_options]);

/**
 * @generated from message loams.collection.v1.SearchRequest
 */
export type SearchRequest = Message<"loams.collection.v1.SearchRequest"> & {
  /**
   * @generated from field: string namespace = 1;
   */
  namespace: string;

  /**
   * A collection name or an alias. The alias of `from` is this field's other
   * name; a request that sets both names one collection, and `collection`
   * wins.
   *
   * @generated from field: string collection = 2;
   */
  collection: string;

  /**
   * How fresh the read must be. Absent is `FRESHNESS_STRONG`, the same default
   * the REST route reads, and the `loams-consistency-token` request header
   * still merges into it by the same rule.
   *
   * @generated from field: loams.collection.v1.Consistency consistency = 3;
   */
  consistency?: Consistency | undefined;

  /**
   * The sources of ranked candidates. Empty with a `filter` is a filter-only
   * query; empty with neither matches nothing.
   *
   * @generated from field: repeated loams.collection.v1.Retriever retrievers = 4;
   */
  retrievers: Retriever[];

  /**
   * How the ranked lists are combined. Absent is the engine's own combination,
   * which is RRF.
   *
   * @generated from field: loams.collection.v1.Fusion fusion = 5;
   */
  fusion?: Fusion | undefined;

  /**
   * The filter every retriever's candidates must match.
   *
   * @generated from field: loams.collection.v1.Query filter = 6;
   */
  filter?: Query | undefined;

  /**
   * The effective sort; the primary key ascending always breaks ties (Ruling
   * 10).
   *
   * @generated from field: repeated loams.collection.v1.SortKey sort = 7;
   */
  sort: SortKey[];

  /**
   * How many hits to skip before the window. `offset` is the IR's, and it is
   * capped by the engine's own limits.
   *
   * @generated from field: uint32 offset = 8;
   */
  offset: number;

  /**
   * The window size; absent is 10, the IR's default. `optional`, so an
   * explicit `0` is a window of nothing rather than the default.
   *
   * @generated from field: optional uint32 limit = 9;
   */
  limit?: number | undefined;

  /**
   * The sort values of the last hit of the previous page, for a deep page
   * jump. Each is the IR's own JSON: `null`, a bool, a number, a string or
   * `{"uuid": "…"}`.
   *
   * @generated from field: repeated google.protobuf.Value search_after = 10;
   */
  searchAfter: Value[];

  /**
   * Hits scoring below this are dropped. `optional`, so `0` is a threshold and
   * not an absent one.
   *
   * @generated from field: optional float score_threshold = 11;
   */
  scoreThreshold?: number | undefined;

  /**
   * What to return of each hit, as the REST route's projection JSON
   * (`{"source": "all" | "none" | {"include": [], "exclude": []},
   * "vectors": [], "fields": []}`). Absent is everything, and `id` and
   * `_score` are answered whatever it says.
   *
   * @generated from field: google.protobuf.Struct select = 12;
   */
  select?: JsonObject | undefined;

  /**
   * An ES aggregation request, carried verbatim as Tantivy's own
   * `Aggregations` JSON owns it.
   *
   * @generated from field: google.protobuf.Value aggregations = 13;
   */
  aggregations?: Value | undefined;

  /**
   * Text fields to highlight, and how.
   *
   * @generated from field: loams.collection.v1.Highlight highlight = 14;
   */
  highlight?: Highlight | undefined;

  /**
   * Groups the hits by a field value instead of answering a flat page.
   *
   * @generated from field: loams.collection.v1.GroupBy group_by = 15;
   */
  groupBy?: GroupBy | undefined;

  /**
   * Whether and how far to count the matches. A `Value` and not a message,
   * for the reason the header gives.
   *
   * @generated from field: google.protobuf.Value track_total_hits = 16;
   */
  trackTotalHits?: Value | undefined;

  /**
   * **Always `invalid_argument`.** Re-scoring arrives in M3; the field exists
   * so a body carrying it is refused rather than silently dropped.
   *
   * @generated from field: google.protobuf.Struct rerank = 17;
   */
  rerank?: JsonObject | undefined;

  /**
   * **Always `invalid_argument`.** Graph expansion arrives in M3; see `rerank`.
   *
   * @generated from field: google.protobuf.Struct expand = 18;
   */
  expand?: JsonObject | undefined;

  /**
   * Design §05 §4's `from`, verbatim: `"collections.<name>"` or `"<name>"`.
   * An alias of `collection` (see the header).
   *
   * @generated from field: string from = 19;
   */
  from: string;

  /**
   * Design §05 §4's `retrieve`, verbatim, same element shape. An alias of
   * `retrievers`.
   *
   * @generated from field: repeated loams.collection.v1.Retriever retrieve = 20;
   */
  retrieve: Retriever[];

  /**
   * Design §05 §4's `fuse`, verbatim: `{"method": "rrf", "k": 60}`. A `Struct`
   * for the reason the header gives, and an alias of `fusion`.
   *
   * @generated from field: google.protobuf.Struct fuse = 21;
   */
  fuse?: JsonObject | undefined;
};

/**
 * Describes the message loams.collection.v1.SearchRequest.
 * Use `create(SearchRequestSchema)` to create a new message.
 */
export const SearchRequestSchema: GenMessage<SearchRequest> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 0);

/**
 * One source of ranked candidates. A proto oneof, so the JSON is the REST
 * route's `{"vector": {…}}` / `{"text": {…}}` / `{"fused": {…}}` /
 * `{"rescore": {…}}` / `{"sparse": {…}}` unchanged, and a retriever that is
 * none of the five is `invalid_argument`.
 *
 * @generated from message loams.collection.v1.Retriever
 */
export type Retriever = Message<"loams.collection.v1.Retriever"> & {
  /**
   * @generated from oneof loams.collection.v1.Retriever.retriever
   */
  retriever: {
    /**
     * @generated from field: loams.collection.v1.VectorQuery vector = 1;
     */
    value: VectorQuery;
    case: "vector";
  } | {
    /**
     * @generated from field: loams.collection.v1.TextQuery text = 2;
     */
    value: TextQuery;
    case: "text";
  } | {
    /**
     * Runs its own inputs and fuses them, which makes it a subtree of
     * retrievers rather than one stage.
     *
     * @generated from field: loams.collection.v1.FusedQuery fused = 3;
     */
    value: FusedQuery;
    case: "fused";
  } | {
    /**
     * Re-ranks an input's candidates against a second vector.
     *
     * @generated from field: loams.collection.v1.RescoreQuery rescore = 4;
     */
    value: RescoreQuery;
    case: "rescore";
  } | {
    /**
     * Exact sparse-vector search (overview A29).
     *
     * @generated from field: loams.collection.v1.SparseQuery sparse = 5;
     */
    value: SparseQuery;
    case: "sparse";
  } | { case: undefined; value?: undefined };
};

/**
 * Describes the message loams.collection.v1.Retriever.
 * Use `create(RetrieverSchema)` to create a new message.
 */
export const RetrieverSchema: GenMessage<Retriever> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 1);

/**
 * @generated from message loams.collection.v1.VectorQuery
 */
export type VectorQuery = Message<"loams.collection.v1.VectorQuery"> & {
  /**
   * The dense vector column to search.
   *
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * The query vector, as `[1.0, 0.0, 0.0]`.
   *
   * @generated from field: repeated float query = 2;
   */
  query: number[];

  /**
   * How many candidates this retriever contributes before fusion.
   *
   * @generated from field: uint32 k = 3;
   */
  k: number;

  /**
   * @generated from field: loams.collection.v1.AnnParams params = 4;
   */
  params?: AnnParams | undefined;

  /**
   * Narrows this retriever's candidates, over and above the request's
   * `filter`.
   *
   * @generated from field: loams.collection.v1.Query filter = 5;
   */
  filter?: Query | undefined;
};

/**
 * Describes the message loams.collection.v1.VectorQuery.
 * Use `create(VectorQuerySchema)` to create a new message.
 */
export const VectorQuerySchema: GenMessage<VectorQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 2);

/**
 * @generated from message loams.collection.v1.TextQuery
 */
export type TextQuery = Message<"loams.collection.v1.TextQuery"> & {
  /**
   * The query over the collection's text fields.
   *
   * @generated from field: loams.collection.v1.Query query = 1;
   */
  query?: Query | undefined;

  /**
   * @generated from field: uint32 k = 2;
   */
  k: number;
};

/**
 * Describes the message loams.collection.v1.TextQuery.
 * Use `create(TextQuerySchema)` to create a new message.
 */
export const TextQuerySchema: GenMessage<TextQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 3);

/**
 * @generated from message loams.collection.v1.FusedQuery
 */
export type FusedQuery = Message<"loams.collection.v1.FusedQuery"> & {
  /**
   * @generated from field: repeated loams.collection.v1.Retriever inputs = 1;
   */
  inputs: Retriever[];

  /**
   * @generated from field: loams.collection.v1.Fusion fusion = 2;
   */
  fusion?: Fusion | undefined;

  /**
   * @generated from field: uint32 k = 3;
   */
  k: number;
};

/**
 * Describes the message loams.collection.v1.FusedQuery.
 * Use `create(FusedQuerySchema)` to create a new message.
 */
export const FusedQuerySchema: GenMessage<FusedQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 4);

/**
 * @generated from message loams.collection.v1.RescoreQuery
 */
export type RescoreQuery = Message<"loams.collection.v1.RescoreQuery"> & {
  /**
   * The retriever whose candidates are re-ranked.
   *
   * @generated from field: loams.collection.v1.Retriever input = 1;
   */
  input?: Retriever | undefined;

  /**
   * The vector column to re-score against.
   *
   * @generated from field: string field = 2;
   */
  field: string;

  /**
   * @generated from field: repeated float query = 3;
   */
  query: number[];

  /**
   * @generated from field: uint32 k = 4;
   */
  k: number;
};

/**
 * Describes the message loams.collection.v1.RescoreQuery.
 * Use `create(RescoreQuerySchema)` to create a new message.
 */
export const RescoreQuerySchema: GenMessage<RescoreQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 5);

/**
 * @generated from message loams.collection.v1.SparseQuery
 */
export type SparseQuery = Message<"loams.collection.v1.SparseQuery"> & {
  /**
   * The sparse vector column to search.
   *
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * The query vector, as the sparse vector's own JSON:
   * `{"indices": [5], "values": [1.0]}`.
   *
   * @generated from field: google.protobuf.Struct query = 2;
   */
  query?: JsonObject | undefined;

  /**
   * @generated from field: uint32 k = 3;
   */
  k: number;

  /**
   * @generated from field: loams.collection.v1.Query filter = 4;
   */
  filter?: Query | undefined;

  /**
   * @generated from field: loams.collection.v1.SparseParams params = 5;
   */
  params?: SparseParams | undefined;
};

/**
 * Describes the message loams.collection.v1.SparseQuery.
 * Use `create(SparseQuerySchema)` to create a new message.
 */
export const SparseQuerySchema: GenMessage<SparseQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 6);

/**
 * The parameters of a dense-vector retriever. Every field absent is the
 * engine's default, which is why they are `optional` rather than sent as
 * zeroes.
 *
 * @generated from message loams.collection.v1.AnnParams
 */
export type AnnParams = Message<"loams.collection.v1.AnnParams"> & {
  /**
   * Search the whole index exactly instead of approximately. This is not a
   * free read.
   *
   * @generated from field: bool exact = 1;
   */
  exact: boolean;

  /**
   * HNSW layers probed.
   *
   * @generated from field: optional uint32 nprobes = 2;
   */
  nprobes?: number | undefined;

  /**
   * Candidates the approximate pass re-ranks exactly.
   *
   * @generated from field: optional uint32 refine_factor = 3;
   */
  refineFactor?: number | undefined;

  /**
   * The approximate search's beam width.
   *
   * @generated from field: optional uint32 ef = 4;
   */
  ef?: number | undefined;

  /**
   * Fractions of each candidate's cells to visit (multi-vector).
   *
   * @generated from field: optional float oversampling = 5;
   */
  oversampling?: number | undefined;

  /**
   * A metric override — `cosine`, `dot`, `euclid` or `manhattan` — as the REST
   * route spells it; only with `exact`. A string rather than an enum so the
   * REST spelling survives verbatim (the same call `ScanColumn.distance`
   * makes).
   *
   * @generated from field: string distance = 6;
   */
  distance: string;
};

/**
 * Describes the message loams.collection.v1.AnnParams.
 * Use `create(AnnParamsSchema)` to create a new message.
 */
export const AnnParamsSchema: GenMessage<AnnParams> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 7);

/**
 * The parameters of a sparse retriever.
 *
 * @generated from message loams.collection.v1.SparseParams
 */
export type SparseParams = Message<"loams.collection.v1.SparseParams"> & {
  /**
   * The rows the IDF statistics count. Absent is every live row.
   *
   * @generated from field: loams.collection.v1.Query idf_corpus = 1;
   */
  idfCorpus?: Query | undefined;
};

/**
 * Describes the message loams.collection.v1.SparseParams.
 * Use `create(SparseParamsSchema)` to create a new message.
 */
export const SparseParamsSchema: GenMessage<SparseParams> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 8);

/**
 * How ranked lists are combined (overview §6.6, Rulings 4 and 5). A proto
 * oneof, so the JSON is the IR's `{"rrf": {"k": 60}}` / `{"dbsf": {}}` /
 * `{"weightedSum": {"weights": [0.5, 0.5]}}` — `dbsf` is a unit arm and so is an
 * empty message, because the REST spelling `"dbsf"` has no oneof form.
 *
 * @generated from message loams.collection.v1.Fusion
 */
export type Fusion = Message<"loams.collection.v1.Fusion"> & {
  /**
   * @generated from oneof loams.collection.v1.Fusion.fusion
   */
  fusion: {
    /**
     * @generated from field: loams.collection.v1.Rrf rrf = 1;
     */
    value: Rrf;
    case: "rrf";
  } | {
    /**
     * Distance-based score fusion, with the engine's own parameters.
     *
     * @generated from field: google.protobuf.Empty dbsf = 2;
     */
    value: Empty;
    case: "dbsf";
  } | {
    /**
     * @generated from field: loams.collection.v1.WeightedSum weighted_sum = 3;
     */
    value: WeightedSum;
    case: "weightedSum";
  } | { case: undefined; value?: undefined };
};

/**
 * Describes the message loams.collection.v1.Fusion.
 * Use `create(FusionSchema)` to create a new message.
 */
export const FusionSchema: GenMessage<Fusion> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 9);

/**
 * @generated from message loams.collection.v1.Rrf
 */
export type Rrf = Message<"loams.collection.v1.Rrf"> & {
  /**
   * The RRF constant; absent is 60, the IR's default, so `optional` rather
   * than a `0` the engine would have to read as "no smoothing at all".
   *
   * @generated from field: optional uint32 k = 1;
   */
  k?: number | undefined;
};

/**
 * Describes the message loams.collection.v1.Rrf.
 * Use `create(RrfSchema)` to create a new message.
 */
export const RrfSchema: GenMessage<Rrf> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 10);

/**
 * @generated from message loams.collection.v1.WeightedSum
 */
export type WeightedSum = Message<"loams.collection.v1.WeightedSum"> & {
  /**
   * One weight per input retriever, in the order the retrievers are given.
   *
   * @generated from field: repeated float weights = 1;
   */
  weights: number[];
};

/**
 * Describes the message loams.collection.v1.WeightedSum.
 * Use `create(WeightedSumSchema)` to create a new message.
 */
export const WeightedSumSchema: GenMessage<WeightedSum> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 11);

/**
 * A query over the collection's fields: a filter, a text retriever's query, or
 * a sparse retriever's IDF corpus. A proto oneof, so the JSON is the IR's own
 * externally-tagged spelling with the one difference proto3 JSON forces: a unit
 * arm is `{"matchAll": {}}` rather than `"match_all"`, and the arms with an
 * underscore in them are `lowerCamelCase` (`matchPhrase`, `multiMatch`,
 * `valuesCount`, `isNull`, `isEmpty`, `queryString`, `constantScore`,
 * `weightedSum` — see the header).
 *
 * @generated from message loams.collection.v1.Query
 */
export type Query = Message<"loams.collection.v1.Query"> & {
  /**
   * @generated from oneof loams.collection.v1.Query.query
   */
  query: {
    /**
     * Every document.
     *
     * @generated from field: google.protobuf.Empty match_all = 1;
     */
    value: Empty;
    case: "matchAll";
  } | {
    /**
     * No document.
     *
     * @generated from field: google.protobuf.Empty match_none = 2;
     */
    value: Empty;
    case: "matchNone";
  } | {
    /**
     * @generated from field: loams.collection.v1.MatchQuery match = 3;
     */
    value: MatchQuery;
    case: "match";
  } | {
    /**
     * @generated from field: loams.collection.v1.MatchPhraseQuery match_phrase = 4;
     */
    value: MatchPhraseQuery;
    case: "matchPhrase";
  } | {
    /**
     * @generated from field: loams.collection.v1.MultiMatchQuery multi_match = 5;
     */
    value: MultiMatchQuery;
    case: "multiMatch";
  } | {
    /**
     * The field equals one value.
     *
     * @generated from field: loams.collection.v1.TermQuery term = 6;
     */
    value: TermQuery;
    case: "term";
  } | {
    /**
     * The field equals any of these values.
     *
     * @generated from field: loams.collection.v1.TermsQuery terms = 7;
     */
    value: TermsQuery;
    case: "terms";
  } | {
    /**
     * @generated from field: loams.collection.v1.RangeQuery range = 8;
     */
    value: RangeQuery;
    case: "range";
  } | {
    /**
     * The field is present, whatever its value.
     *
     * @generated from field: loams.collection.v1.FieldQuery exists = 9;
     */
    value: FieldQuery;
    case: "exists";
  } | {
    /**
     * @generated from field: loams.collection.v1.FieldQuery is_null = 10;
     */
    value: FieldQuery;
    case: "isNull";
  } | {
    /**
     * @generated from field: loams.collection.v1.FieldQuery is_empty = 11;
     */
    value: FieldQuery;
    case: "isEmpty";
  } | {
    /**
     * @generated from field: loams.collection.v1.ValuesCountQuery values_count = 12;
     */
    value: ValuesCountQuery;
    case: "valuesCount";
  } | {
    /**
     * @generated from field: loams.collection.v1.PrefixQuery prefix = 13;
     */
    value: PrefixQuery;
    case: "prefix";
  } | {
    /**
     * @generated from field: loams.collection.v1.WildcardQuery wildcard = 14;
     */
    value: WildcardQuery;
    case: "wildcard";
  } | {
    /**
     * @generated from field: loams.collection.v1.FuzzyQuery fuzzy = 15;
     */
    value: FuzzyQuery;
    case: "fuzzy";
  } | {
    /**
     * @generated from field: loams.collection.v1.QueryStringQuery query_string = 17;
     */
    value: QueryStringQuery;
    case: "queryString";
  } | {
    /**
     * @generated from field: loams.collection.v1.BoolQuery bool = 18;
     */
    value: BoolQuery;
    case: "bool";
  } | {
    /**
     * @generated from field: loams.collection.v1.BoostQuery boost = 19;
     */
    value: BoostQuery;
    case: "boost";
  } | {
    /**
     * @generated from field: loams.collection.v1.ConstantScoreQuery constant_score = 20;
     */
    value: ConstantScoreQuery;
    case: "constantScore";
  } | { case: undefined; value?: undefined };

  /**
   * The documents with these ids, and no others.
   *
   * **Not a oneof arm**, and the reason is a wire spelling rather than a
   * modelling preference: a proto3 JSON oneof arm is always `{"arm": {…}}`, and
   * the value here is a **list**, so an arm could only be spelled
   * `{"ids": {"ids": [1, 2]}}` — a wrapper around one field that no IR has and a
   * caller would have to learn separately. As a plain field the list sits exactly
   * where the REST IR puts it: `{"ids": [1, {"uuid": "…"}]}`, which is
   * `{"ids": [{"uint": "1"}, {"uuid": "…"}]}` in proto3 JSON.
   *
   * So `Query` is a oneof **plus** this field, and a request that sets both is
   * `invalid_argument` rather than letting one silently win: two keys naming two
   * different filters is a request that cannot be answered. That refusal is the
   * handler's, not the codec's, and it names the field.
   *
   * Each element is a `google.protobuf.Value`, not a `DocumentId`, because a
   * `repeated` field cannot be a oneof arm in protobuf at all, and because the
   * `DocumentId` shape `{"uint": "1"}` **is** a `Value` — a one-key struct whose
   * values are strings, so a `u64::MAX` id stays exact and `{"uuid": "…"}` keeps
   * the REST spelling verbatim. The handler reads each back through the same
   * `loams_query::json::pk::from_json` `GetDocumentsRequest.ids` goes through, so
   * the three arms and their refusals are the document RPCs'.
   *
   * @generated from field: repeated google.protobuf.Value ids = 16;
   */
  ids: Value[];
};

/**
 * Describes the message loams.collection.v1.Query.
 * Use `create(QuerySchema)` to create a new message.
 */
export const QuerySchema: GenMessage<Query> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 12);

/**
 * @generated from message loams.collection.v1.MatchQuery
 */
export type MatchQuery = Message<"loams.collection.v1.MatchQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * @generated from field: string text = 2;
   */
  text: string;

  /**
   * Whether every term must match: `or` or `and`, absent being `or`.
   *
   * A **string**, not the `BoolOperator` enum, and this is the one enum-shaped
   * field in the file that is not one: the IR's spelling is `"or"` / `"and"`
   * and that is what §05 §4's example body and the native REST route both
   * write, so an enum here would have the codec refuse `"operator": "or"` as an
   * unknown variant *before a handler runs* — a refusal with no reason and no
   * field. A string keeps the REST spelling verbatim, which is the same call
   * `Patch.mode` and `ScanColumn.role` make for the same reason; `or` and `and`
   * are the accepted values and anything else is `invalid_argument`.
   *
   * @generated from field: string operator = 3;
   */
  operator: string;

  /**
   * How many of the terms must match, as the IR's own string form (`"2"`,
   * `"75%"`).
   *
   * @generated from field: optional string minimum_should_match = 4;
   */
  minimumShouldMatch?: string | undefined;

  /**
   * @generated from field: loams.collection.v1.Fuzziness fuzziness = 5;
   */
  fuzziness?: Fuzziness | undefined;

  /**
   * The analyzer to run, over the field's own.
   *
   * @generated from field: string analyzer = 6;
   */
  analyzer: string;
};

/**
 * Describes the message loams.collection.v1.MatchQuery.
 * Use `create(MatchQuerySchema)` to create a new message.
 */
export const MatchQuerySchema: GenMessage<MatchQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 13);

/**
 * @generated from message loams.collection.v1.MatchPhraseQuery
 */
export type MatchPhraseQuery = Message<"loams.collection.v1.MatchPhraseQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * @generated from field: string text = 2;
   */
  text: string;

  /**
   * How far apart the terms may be and still match, in positions.
   *
   * @generated from field: uint32 slop = 3;
   */
  slop: number;
};

/**
 * Describes the message loams.collection.v1.MatchPhraseQuery.
 * Use `create(MatchPhraseQuerySchema)` to create a new message.
 */
export const MatchPhraseQuerySchema: GenMessage<MatchPhraseQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 14);

/**
 * @generated from message loams.collection.v1.MultiMatchQuery
 */
export type MultiMatchQuery = Message<"loams.collection.v1.MultiMatchQuery"> & {
  /**
   * The fields to search, each with its boost.
   *
   * @generated from field: repeated loams.collection.v1.MultiMatchField fields = 1;
   */
  fields: MultiMatchField[];

  /**
   * @generated from field: string text = 2;
   */
  text: string;

  /**
   * How the fields are combined; absent is `MULTI_MATCH_KIND_BEST_FIELDS`.
   *
   * @generated from field: loams.collection.v1.MultiMatchKind kind = 3;
   */
  kind: MultiMatchKind;

  /**
   * @generated from field: loams.collection.v1.BoolOperator operator = 4;
   */
  operator: BoolOperator;

  /**
   * How much a best-fields match counts against a most-fields one; absent is
   * the engine's own.
   *
   * @generated from field: optional float tie_breaker = 5;
   */
  tieBreaker?: number | undefined;
};

/**
 * Describes the message loams.collection.v1.MultiMatchQuery.
 * Use `create(MultiMatchQuerySchema)` to create a new message.
 */
export const MultiMatchQuerySchema: GenMessage<MultiMatchQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 15);

/**
 * @generated from message loams.collection.v1.MultiMatchField
 */
export type MultiMatchField = Message<"loams.collection.v1.MultiMatchField"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * @generated from field: float boost = 2;
   */
  boost: number;
};

/**
 * Describes the message loams.collection.v1.MultiMatchField.
 * Use `create(MultiMatchFieldSchema)` to create a new message.
 */
export const MultiMatchFieldSchema: GenMessage<MultiMatchField> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 16);

/**
 * @generated from message loams.collection.v1.TermQuery
 */
export type TermQuery = Message<"loams.collection.v1.TermQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * The value, as the IR's own JSON: a string, a bool, a number or
   * `{"date": "<RFC 3339>"}` (Ruling 9).
   *
   * @generated from field: google.protobuf.Value value = 2;
   */
  value?: Value | undefined;
};

/**
 * Describes the message loams.collection.v1.TermQuery.
 * Use `create(TermQuerySchema)` to create a new message.
 */
export const TermQuerySchema: GenMessage<TermQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 17);

/**
 * @generated from message loams.collection.v1.TermsQuery
 */
export type TermsQuery = Message<"loams.collection.v1.TermsQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * @generated from field: repeated google.protobuf.Value values = 2;
   */
  values: Value[];
};

/**
 * Describes the message loams.collection.v1.TermsQuery.
 * Use `create(TermsQuerySchema)` to create a new message.
 */
export const TermsQuerySchema: GenMessage<TermsQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 18);

/**
 * @generated from message loams.collection.v1.RangeQuery
 */
export type RangeQuery = Message<"loams.collection.v1.RangeQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * The bounds, each a value of the field's own kind and each optional: an
   * absent bound is unbounded on that side. `gt`/`gte` and `lt`/`lte` both set
   * is `invalid_argument`.
   *
   * @generated from field: google.protobuf.Value gt = 2;
   */
  gt?: Value | undefined;

  /**
   * @generated from field: google.protobuf.Value gte = 3;
   */
  gte?: Value | undefined;

  /**
   * @generated from field: google.protobuf.Value lt = 4;
   */
  lt?: Value | undefined;

  /**
   * @generated from field: google.protobuf.Value lte = 5;
   */
  lte?: Value | undefined;
};

/**
 * Describes the message loams.collection.v1.RangeQuery.
 * Use `create(RangeQuerySchema)` to create a new message.
 */
export const RangeQuerySchema: GenMessage<RangeQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 19);

/**
 * `exists`, `is_null` and `is_empty`: one message, because a caller asks the
 * same question of a field in each case.
 *
 * @generated from message loams.collection.v1.FieldQuery
 */
export type FieldQuery = Message<"loams.collection.v1.FieldQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;
};

/**
 * Describes the message loams.collection.v1.FieldQuery.
 * Use `create(FieldQuerySchema)` to create a new message.
 */
export const FieldQuerySchema: GenMessage<FieldQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 20);

/**
 * How many values a field's array (or token) column holds.
 *
 * @generated from message loams.collection.v1.ValuesCountQuery
 */
export type ValuesCountQuery = Message<"loams.collection.v1.ValuesCountQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * @generated from field: optional uint64 gt = 2;
   */
  gt?: bigint | undefined;

  /**
   * @generated from field: optional uint64 gte = 3;
   */
  gte?: bigint | undefined;

  /**
   * @generated from field: optional uint64 lt = 4;
   */
  lt?: bigint | undefined;

  /**
   * @generated from field: optional uint64 lte = 5;
   */
  lte?: bigint | undefined;
};

/**
 * Describes the message loams.collection.v1.ValuesCountQuery.
 * Use `create(ValuesCountQuerySchema)` to create a new message.
 */
export const ValuesCountQuerySchema: GenMessage<ValuesCountQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 21);

/**
 * @generated from message loams.collection.v1.PrefixQuery
 */
export type PrefixQuery = Message<"loams.collection.v1.PrefixQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * @generated from field: string value = 2;
   */
  value: string;
};

/**
 * Describes the message loams.collection.v1.PrefixQuery.
 * Use `create(PrefixQuerySchema)` to create a new message.
 */
export const PrefixQuerySchema: GenMessage<PrefixQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 22);

/**
 * @generated from message loams.collection.v1.WildcardQuery
 */
export type WildcardQuery = Message<"loams.collection.v1.WildcardQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * With `*` and `?`, as the REST route spells it.
   *
   * @generated from field: string pattern = 2;
   */
  pattern: string;
};

/**
 * Describes the message loams.collection.v1.WildcardQuery.
 * Use `create(WildcardQuerySchema)` to create a new message.
 */
export const WildcardQuerySchema: GenMessage<WildcardQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 23);

/**
 * @generated from message loams.collection.v1.FuzzyQuery
 */
export type FuzzyQuery = Message<"loams.collection.v1.FuzzyQuery"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * @generated from field: string value = 2;
   */
  value: string;

  /**
   * @generated from field: loams.collection.v1.Fuzziness fuzziness = 3;
   */
  fuzziness?: Fuzziness | undefined;
};

/**
 * Describes the message loams.collection.v1.FuzzyQuery.
 * Use `create(FuzzyQuerySchema)` to create a new message.
 */
export const FuzzyQuerySchema: GenMessage<FuzzyQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 24);

/**
 * @generated from message loams.collection.v1.QueryStringQuery
 */
export type QueryStringQuery = Message<"loams.collection.v1.QueryStringQuery"> & {
  /**
   * The Lucene query language, as the REST route's `query_string` takes it.
   *
   * @generated from field: string query = 1;
   */
  query: string;

  /**
   * The fields to search when the query names none.
   *
   * @generated from field: repeated string default_fields = 2;
   */
  defaultFields: string[];

  /**
   * @generated from field: loams.collection.v1.BoolOperator default_operator = 3;
   */
  defaultOperator: BoolOperator;
};

/**
 * Describes the message loams.collection.v1.QueryStringQuery.
 * Use `create(QueryStringQuerySchema)` to create a new message.
 */
export const QueryStringQuerySchema: GenMessage<QueryStringQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 25);

/**
 * @generated from message loams.collection.v1.BoolQuery
 */
export type BoolQuery = Message<"loams.collection.v1.BoolQuery"> & {
  /**
   * Every one of these must match.
   *
   * @generated from field: repeated loams.collection.v1.Query must = 1;
   */
  must: Query[];

  /**
   * At least `minimum_should_match` of these must match, and scoring is their
   * sum.
   *
   * @generated from field: repeated loams.collection.v1.Query should = 2;
   */
  should: Query[];

  /**
   * None of these may match.
   *
   * @generated from field: repeated loams.collection.v1.Query must_not = 3;
   */
  mustNot: Query[];

  /**
   * These must match and do not score.
   *
   * @generated from field: repeated loams.collection.v1.Query filter = 4;
   */
  filter: Query[];

  /**
   * @generated from field: optional string minimum_should_match = 5;
   */
  minimumShouldMatch?: string | undefined;
};

/**
 * Describes the message loams.collection.v1.BoolQuery.
 * Use `create(BoolQuerySchema)` to create a new message.
 */
export const BoolQuerySchema: GenMessage<BoolQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 26);

/**
 * @generated from message loams.collection.v1.BoostQuery
 */
export type BoostQuery = Message<"loams.collection.v1.BoostQuery"> & {
  /**
   * The query whose score is multiplied.
   *
   * @generated from field: loams.collection.v1.Query query = 1;
   */
  query?: Query | undefined;

  /**
   * @generated from field: float boost = 2;
   */
  boost: number;
};

/**
 * Describes the message loams.collection.v1.BoostQuery.
 * Use `create(BoostQuerySchema)` to create a new message.
 */
export const BoostQuerySchema: GenMessage<BoostQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 27);

/**
 * @generated from message loams.collection.v1.ConstantScoreQuery
 */
export type ConstantScoreQuery = Message<"loams.collection.v1.ConstantScoreQuery"> & {
  /**
   * The query that decides membership; its own score is discarded.
   *
   * @generated from field: loams.collection.v1.Query query = 1;
   */
  query?: Query | undefined;

  /**
   * The score every match is given.
   *
   * @generated from field: float score = 2;
   */
  score: number;
};

/**
 * Describes the message loams.collection.v1.ConstantScoreQuery.
 * Use `create(ConstantScoreQuerySchema)` to create a new message.
 */
export const ConstantScoreQuerySchema: GenMessage<ConstantScoreQuery> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 28);

/**
 * How far a fuzzy match may edit. A oneof, so the REST spellings `"auto"` and
 * an integer 0–2 become `{"auto": {}}` and `{"edits": 1}` — a bare string has
 * no oneof form, the same reason `match_all` is an empty message.
 *
 * @generated from message loams.collection.v1.Fuzziness
 */
export type Fuzziness = Message<"loams.collection.v1.Fuzziness"> & {
  /**
   * @generated from oneof loams.collection.v1.Fuzziness.fuzziness
   */
  fuzziness: {
    /**
     * The engine picks from the term's length.
     *
     * @generated from field: google.protobuf.Empty auto = 1;
     */
    value: Empty;
    case: "auto";
  } | {
    /**
     * An exact number of edits, 0 to 2.
     *
     * @generated from field: uint32 edits = 2;
     */
    value: number;
    case: "edits";
  } | { case: undefined; value?: undefined };
};

/**
 * Describes the message loams.collection.v1.Fuzziness.
 * Use `create(FuzzinessSchema)` to create a new message.
 */
export const FuzzinessSchema: GenMessage<Fuzziness> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 29);

/**
 * One key of the effective sort; the primary key ascending always breaks ties
 * (Ruling 10). A proto oneof, so the JSON is `{"score": {"order": …}}` /
 * `{"pk": {"order": …}}` / `{"field": {"field": …, "order": …,
 * "missing": …}}` — the same shape the IR's serde produces, and
 * `{"pk": {}}` remains a valid way to say "primary key, ascending".
 *
 * @generated from message loams.collection.v1.SortKey
 */
export type SortKey = Message<"loams.collection.v1.SortKey"> & {
  /**
   * @generated from oneof loams.collection.v1.SortKey.key
   */
  key: {
    /**
     * @generated from field: loams.collection.v1.ScoreSort score = 1;
     */
    value: ScoreSort;
    case: "score";
  } | {
    /**
     * @generated from field: loams.collection.v1.PrimaryKeySort pk = 2;
     */
    value: PrimaryKeySort;
    case: "pk";
  } | {
    /**
     * @generated from field: loams.collection.v1.FieldSort field = 3;
     */
    value: FieldSort;
    case: "field";
  } | { case: undefined; value?: undefined };
};

/**
 * Describes the message loams.collection.v1.SortKey.
 * Use `create(SortKeySchema)` to create a new message.
 */
export const SortKeySchema: GenMessage<SortKey> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 30);

/**
 * @generated from message loams.collection.v1.ScoreSort
 */
export type ScoreSort = Message<"loams.collection.v1.ScoreSort"> & {
  /**
   * Absent is descending, which is what sorting by score means.
   *
   * @generated from field: loams.collection.v1.SortOrder order = 1;
   */
  order: SortOrder;
};

/**
 * Describes the message loams.collection.v1.ScoreSort.
 * Use `create(ScoreSortSchema)` to create a new message.
 */
export const ScoreSortSchema: GenMessage<ScoreSort> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 31);

/**
 * @generated from message loams.collection.v1.PrimaryKeySort
 */
export type PrimaryKeySort = Message<"loams.collection.v1.PrimaryKeySort"> & {
  /**
   * Absent is ascending.
   *
   * @generated from field: loams.collection.v1.SortOrder order = 1;
   */
  order: SortOrder;
};

/**
 * Describes the message loams.collection.v1.PrimaryKeySort.
 * Use `create(PrimaryKeySortSchema)` to create a new message.
 */
export const PrimaryKeySortSchema: GenMessage<PrimaryKeySort> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 32);

/**
 * @generated from message loams.collection.v1.FieldSort
 */
export type FieldSort = Message<"loams.collection.v1.FieldSort"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * Absent is ascending.
   *
   * @generated from field: loams.collection.v1.SortOrder order = 2;
   */
  order: SortOrder;

  /**
   * Where a document without the field sorts. Absent is last.
   *
   * @generated from field: loams.collection.v1.MissingOrder missing = 3;
   */
  missing: MissingOrder;
};

/**
 * Describes the message loams.collection.v1.FieldSort.
 * Use `create(FieldSortSchema)` to create a new message.
 */
export const FieldSortSchema: GenMessage<FieldSort> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 33);

/**
 * Highlighting of text fields (ES `highlight`).
 *
 * @generated from message loams.collection.v1.Highlight
 */
export type Highlight = Message<"loams.collection.v1.Highlight"> & {
  /**
   * @generated from field: repeated loams.collection.v1.HighlightField fields = 1;
   */
  fields: HighlightField[];
};

/**
 * Describes the message loams.collection.v1.Highlight.
 * Use `create(HighlightSchema)` to create a new message.
 */
export const HighlightSchema: GenMessage<Highlight> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 34);

/**
 * @generated from message loams.collection.v1.HighlightField
 */
export type HighlightField = Message<"loams.collection.v1.HighlightField"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * What wraps a matched term. Absent is the IR's `<em>` / `</em>`, which is
   * why both are `optional`: proto3 cannot tell an absent string from `""`,
   * and `""` is a legitimate tag.
   *
   * @generated from field: optional string pre_tag = 2;
   */
  preTag?: string | undefined;

  /**
   * @generated from field: optional string post_tag = 3;
   */
  postTag?: string | undefined;

  /**
   * The size of one fragment in characters; absent is the IR's 100.
   *
   * @generated from field: optional uint32 fragment_size = 4;
   */
  fragmentSize?: number | undefined;

  /**
   * How many fragments of the field to answer; absent is the IR's 5.
   *
   * @generated from field: optional uint32 number_of_fragments = 5;
   */
  numberOfFragments?: number | undefined;
};

/**
 * Describes the message loams.collection.v1.HighlightField.
 * Use `create(HighlightFieldSchema)` to create a new message.
 */
export const HighlightFieldSchema: GenMessage<HighlightField> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 35);

/**
 * Groups hits by a field value (Qdrant's `group_by`).
 *
 * @generated from message loams.collection.v1.GroupBy
 */
export type GroupBy = Message<"loams.collection.v1.GroupBy"> & {
  /**
   * @generated from field: string field = 1;
   */
  field: string;

  /**
   * How many hits of a group to answer; absent is the IR's 3.
   *
   * @generated from field: optional uint32 group_size = 2;
   */
  groupSize?: number | undefined;

  /**
   * How many groups to answer; absent is the IR's 10.
   *
   * @generated from field: optional uint32 limit = 3;
   */
  limit?: number | undefined;
};

/**
 * Describes the message loams.collection.v1.GroupBy.
 * Use `create(GroupBySchema)` to create a new message.
 */
export const GroupBySchema: GenMessage<GroupBy> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 36);

/**
 * @generated from message loams.collection.v1.SearchResponse
 */
export type SearchResponse = Message<"loams.collection.v1.SearchResponse"> & {
  /**
   * The hits, in the effective order. Absent when there are none, which is
   * proto3 JSON's spelling of an empty list.
   *
   * @generated from field: repeated loams.collection.v1.Hit hits = 1;
   */
  hits: Hit[];

  /**
   * How many documents matched. Absent unless `track_total_hits` asked for a
   * count, because counting is not free.
   *
   * @generated from field: loams.collection.v1.TotalHits total = 2;
   */
  total?: TotalHits | undefined;

  /**
   * The aggregations, as Tantivy's own JSON answers them.
   *
   * @generated from field: google.protobuf.Value aggregations = 3;
   */
  aggregations?: Value | undefined;

  /**
   * The groups, when the request asked to group.
   *
   * @generated from field: repeated loams.collection.v1.HitGroup groups = 4;
   */
  groups: HitGroup[];

  /**
   * The state the read saw (overview §6.5), and a `consistency.at_least` that
   * reads this state again. The same value is the `loams-consistency-token`
   * response header.
   *
   * @generated from field: string read_token = 5;
   */
  readToken: string;

  /**
   * The hot structures the read used — `hnsw`, `splits` — in the REST route's
   * spelling and in declaration order, and **absent when it used none**. The
   * same list is the `loams-hot-used` response header, which answers `none`
   * where this answers nothing at all: the header is always there, this field
   * is only there when there is something in it.
   *
   * @generated from field: repeated string hot_used = 6;
   */
  hotUsed: string[];

  /**
   * The design's per-answer `performance` block (§05 §4). Nothing computes it
   * yet, on either surface, so it is declared here and never answered: the
   * field names the contract this answer will meet rather than reporting one
   * this build does not measure. A REST answer has no such key today, and
   * adding it to one surface only is exactly what this package's equality test
   * exists to prevent.
   *
   * @generated from field: google.protobuf.Struct performance = 7;
   */
  performance?: JsonObject | undefined;
};

/**
 * Describes the message loams.collection.v1.SearchResponse.
 * Use `create(SearchResponseSchema)` to create a new message.
 */
export const SearchResponseSchema: GenMessage<SearchResponse> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 37);

/**
 * One hit.
 *
 * @generated from message loams.collection.v1.Hit
 */
export type Hit = Message<"loams.collection.v1.Hit"> & {
  /**
   * The document's id, as the `DocumentId` oneof `Document.id` already uses,
   * for Task 3's reason: a `u64::MAX` id must not come back through a
   * `double`.
   *
   * @generated from field: loams.collection.v1.DocumentId pk = 1;
   */
  pk?: DocumentId | undefined;

  /**
   * Absent at zero, which proto3 JSON spells the same way the REST route
   * writes `0.0`: a filter-only answer scores nothing.
   *
   * @generated from field: float score = 2;
   */
  score: number;

  /**
   * The values this hit sorts by, in `sort`'s order, so a caller can resume
   * from it with `search_after`.
   *
   * @generated from field: repeated google.protobuf.Value sort_values = 3;
   */
  sortValues: Value[];

  /**
   * Absent when the projection leaves the source out, rather than an empty
   * object: "no source" and "an empty source" are different answers.
   *
   * @generated from field: google.protobuf.Struct source = 4;
   */
  source?: JsonObject | undefined;

  /**
   * Dense vectors by name, as the REST route's JSON (`[1.0, 0.0, 0.0]`).
   *
   * @generated from field: map<string, google.protobuf.Value> vectors = 5;
   */
  vectors: { [key: string]: Value };

  /**
   * Sparse vectors by name, as the REST route's JSON
   * (`{"indices": [1], "values": [1.0]}`), with `indices` ascending.
   *
   * @generated from field: map<string, google.protobuf.Struct> sparse_vectors = 6;
   */
  sparseVectors: { [key: string]: JsonObject };

  /**
   * The fragments per field, when `highlight` asked for them. A `Value`
   * because it is a list of strings, which proto3 JSON spells as a `Value`'s
   * list.
   *
   * @generated from field: map<string, google.protobuf.Value> highlight = 7;
   */
  highlight: { [key: string]: Value };

  /**
   * The typed field values per name, as the REST route's JSON (Ruling 9).
   *
   * @generated from field: map<string, google.protobuf.Value> fields = 8;
   */
  fields: { [key: string]: Value };
};

/**
 * Describes the message loams.collection.v1.Hit.
 * Use `create(HitSchema)` to create a new message.
 */
export const HitSchema: GenMessage<Hit> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 38);

/**
 * One group of a grouped search.
 *
 * @generated from message loams.collection.v1.HitGroup
 */
export type HitGroup = Message<"loams.collection.v1.HitGroup"> & {
  /**
   * The field value the group is, in the IR's own JSON.
   *
   * @generated from field: google.protobuf.Value key = 1;
   */
  key?: Value | undefined;

  /**
   * @generated from field: repeated loams.collection.v1.Hit hits = 2;
   */
  hits: Hit[];
};

/**
 * Describes the message loams.collection.v1.HitGroup.
 * Use `create(HitGroupSchema)` to create a new message.
 */
export const HitGroupSchema: GenMessage<HitGroup> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 39);

/**
 * How many documents matched, and whether the count is exact.
 *
 * @generated from message loams.collection.v1.TotalHits
 */
export type TotalHits = Message<"loams.collection.v1.TotalHits"> & {
  /**
   * The count. `optional` and always set: a count of zero is a count, and
   * proto3 JSON omitting it would read as "unanswered".
   *
   * @generated from field: optional uint64 value = 1;
   */
  value?: bigint | undefined;

  /**
   * `TOTAL_RELATION_EQ` when `value` is the whole count and
   * `TOTAL_RELATION_GTE` when it is a floor (`track_total_hits` capped at
   * `up_to`).
   *
   * @generated from field: loams.collection.v1.TotalRelation relation = 2;
   */
  relation: TotalRelation;
};

/**
 * Describes the message loams.collection.v1.TotalHits.
 * Use `create(TotalHitsSchema)` to create a new message.
 */
export const TotalHitsSchema: GenMessage<TotalHits> = /*@__PURE__*/
  messageDesc(file_loams_collection_v1_query, 40);

/**
 * `or` / `and`, as the IR spells them.
 *
 * Declared for the two fields whose operator the IR spells differently from the
 * fields above: `MultiMatchQuery.operator` and
 * `QueryStringQuery.default_operator`, which keep this enum and therefore
 * answer (and are sent) `BOOL_OPERATOR_OR` / `BOOL_OPERATOR_AND`.
 *
 * @generated from enum loams.collection.v1.BoolOperator
 */
export enum BoolOperator {
  /**
   * `or`, which is the IR's default for every operator that defaults.
   *
   * @generated from enum value: BOOL_OPERATOR_UNSPECIFIED = 0;
   */
  UNSPECIFIED = 0,

  /**
   * @generated from enum value: BOOL_OPERATOR_OR = 1;
   */
  OR = 1,

  /**
   * @generated from enum value: BOOL_OPERATOR_AND = 2;
   */
  AND = 2,
}

/**
 * Describes the enum loams.collection.v1.BoolOperator.
 */
export const BoolOperatorSchema: GenEnum<BoolOperator> = /*@__PURE__*/
  enumDesc(file_loams_collection_v1_query, 0);

/**
 * @generated from enum loams.collection.v1.MultiMatchKind
 */
export enum MultiMatchKind {
  /**
   * `best_fields`, the IR's default.
   *
   * @generated from enum value: MULTI_MATCH_KIND_UNSPECIFIED = 0;
   */
  UNSPECIFIED = 0,

  /**
   * @generated from enum value: MULTI_MATCH_KIND_BEST_FIELDS = 1;
   */
  BEST_FIELDS = 1,

  /**
   * @generated from enum value: MULTI_MATCH_KIND_MOST_FIELDS = 2;
   */
  MOST_FIELDS = 2,

  /**
   * @generated from enum value: MULTI_MATCH_KIND_CROSS_FIELDS = 3;
   */
  CROSS_FIELDS = 3,

  /**
   * @generated from enum value: MULTI_MATCH_KIND_PHRASE = 4;
   */
  PHRASE = 4,

  /**
   * @generated from enum value: MULTI_MATCH_KIND_PHRASE_PREFIX = 5;
   */
  PHRASE_PREFIX = 5,
}

/**
 * Describes the enum loams.collection.v1.MultiMatchKind.
 */
export const MultiMatchKindSchema: GenEnum<MultiMatchKind> = /*@__PURE__*/
  enumDesc(file_loams_collection_v1_query, 1);

/**
 * @generated from enum loams.collection.v1.SortOrder
 */
export enum SortOrder {
  /**
   * Ascending for a key and a field, descending for a score; each is the IR's
   * default for that key.
   *
   * @generated from enum value: SORT_ORDER_UNSPECIFIED = 0;
   */
  UNSPECIFIED = 0,

  /**
   * @generated from enum value: SORT_ORDER_ASC = 1;
   */
  ASC = 1,

  /**
   * @generated from enum value: SORT_ORDER_DESC = 2;
   */
  DESC = 2,
}

/**
 * Describes the enum loams.collection.v1.SortOrder.
 */
export const SortOrderSchema: GenEnum<SortOrder> = /*@__PURE__*/
  enumDesc(file_loams_collection_v1_query, 2);

/**
 * @generated from enum loams.collection.v1.MissingOrder
 */
export enum MissingOrder {
  /**
   * Last, which is the IR's default.
   *
   * @generated from enum value: MISSING_ORDER_UNSPECIFIED = 0;
   */
  UNSPECIFIED = 0,

  /**
   * @generated from enum value: MISSING_ORDER_FIRST = 1;
   */
  FIRST = 1,

  /**
   * @generated from enum value: MISSING_ORDER_LAST = 2;
   */
  LAST = 2,
}

/**
 * Describes the enum loams.collection.v1.MissingOrder.
 */
export const MissingOrderSchema: GenEnum<MissingOrder> = /*@__PURE__*/
  enumDesc(file_loams_collection_v1_query, 3);

/**
 * @generated from enum loams.collection.v1.TotalRelation
 */
export enum TotalRelation {
  /**
   * @generated from enum value: TOTAL_RELATION_UNSPECIFIED = 0;
   */
  UNSPECIFIED = 0,

  /**
   * `eq`: the count is exact.
   *
   * @generated from enum value: TOTAL_RELATION_EQ = 1;
   */
  EQ = 1,

  /**
   * `gte`: at least this many matched.
   *
   * @generated from enum value: TOTAL_RELATION_GTE = 2;
   */
  GTE = 2,
}

/**
 * Describes the enum loams.collection.v1.TotalRelation.
 */
export const TotalRelationSchema: GenEnum<TotalRelation> = /*@__PURE__*/
  enumDesc(file_loams_collection_v1_query, 4);

/**
 * @generated from service loams.collection.v1.QueryService
 */
export const QueryService: GenService<{
  /**
   * One search over one collection. The whole IR is this request: the
   * retrievers to run, how to fuse them, the filter, the sort and the window.
   *
   * It is `NO_SIDE_EFFECTS` — it reads and counts — so an SDK may retry it
   * after a lost answer. It is still a `POST`, because a search is not free and
   * the request is a body.
   *
   * @generated from rpc loams.collection.v1.QueryService.Search
   */
  search: {
    methodKind: "unary";
    input: typeof SearchRequestSchema;
    output: typeof SearchResponseSchema;
  },
}> = /*@__PURE__*/
  serviceDesc(file_loams_collection_v1_query, 0);

