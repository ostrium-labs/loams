// The filter builder (SDKP §2.4): one verb per arm of `loams.collection.v1.Query`.
//
//   Filter.and(Filter.eq('tags', 'policy'), Filter.range('n', { gte: 2 }))
//
// Every verb is a pure function that returns the arm's **proto3 JSON**, which
// is also how the builder corpus (`sdks/fixtures/builders/*.json`) spells its
// expectations, so one case is one comparison. `toQuery` decodes it into the
// generated message when a caller wants the typed form; the search builder and
// the document calls do that for you.
//
// Values are JSON: a string, a number, a bool, `null`, or the IR's tagged
// objects (`{"date": "<RFC 3339>"}`, `{"uuid": "…"}`).

import { fromJson, type JsonObject, type JsonValue } from '@bufbuild/protobuf';
import { QuerySchema, type Query } from '@loams/proto/query';

/** One filter, as the proto3 JSON of a `loams.collection.v1.Query`. */
export type FilterJson = JsonObject;

/** A document id, as a filter or a document call takes it. A `number` must be
 * a safe integer; pass a `bigint` above 2^53. */
export type DocId = number | bigint | string | { uuid: string } | { uint: string | number | bigint };

/** `{"auto": {}}` or an exact number of edits, 0 to 2. */
export type Fuzziness = 'auto' | 0 | 1 | 2;

function fuzziness(value: Fuzziness): JsonObject {
  return value === 'auto' ? { auto: {} } : { edits: value };
}

/** A document id as the `Query.ids` `Value` spells it: `{"uint": "1"}`,
 * `{"uuid": "…"}` or a bare string. */
export function idValue(id: DocId): JsonValue {
  if (typeof id === 'number') {
    if (!Number.isSafeInteger(id) || id < 0) {
      throw new RangeError(`a numeric document id must be a non-negative safe integer, not ${id}; pass a bigint`);
    }
    return { uint: String(id) };
  }
  if (typeof id === 'bigint') {
    return { uint: id.toString() };
  }
  if (typeof id === 'string') {
    return id;
  }
  if ('uuid' in id) {
    return { uuid: id.uuid };
  }
  return { uint: String(id.uint) };
}

/** The bounds of a range: any of `gt`, `gte`, `lt`, `lte`. */
export interface Bounds {
  gt?: JsonValue;
  gte?: JsonValue;
  lt?: JsonValue;
  lte?: JsonValue;
}

/** The options of a `match`. */
export interface MatchOptions {
  /** `'or'` (the default) or `'and'`. */
  operator?: 'or' | 'and';
  minimumShouldMatch?: string | number;
  fuzziness?: Fuzziness;
  analyzer?: string;
}

/** The options of a `query_string`. */
export interface QueryStringOptions {
  fields?: string[];
  operator?: 'or' | 'and';
}

/** The options of a `multi_match`. */
export interface MultiMatchOptions {
  /** `best_fields` (the default), `most_fields`, `cross_fields`, `phrase`,
   * `phrase_prefix`. */
  kind?: 'best_fields' | 'most_fields' | 'cross_fields' | 'phrase' | 'phrase_prefix';
  operator?: 'or' | 'and';
  tieBreaker?: number;
}

function definedBounds(bounds: Bounds): JsonObject {
  const out: JsonObject = {};
  for (const key of ['gt', 'gte', 'lt', 'lte'] as const) {
    if (bounds[key] !== undefined) {
      out[key] = bounds[key] as JsonValue;
    }
  }
  return out;
}

function boolOperator(operator: 'or' | 'and'): string {
  return operator === 'and' ? 'BOOL_OPERATOR_AND' : 'BOOL_OPERATOR_OR';
}

/** A field name and an optional `^boost`, for `multi_match`. */
function multiMatchField(spec: string): JsonObject {
  const at = spec.lastIndexOf('^');
  if (at > 0) {
    const boost = Number(spec.slice(at + 1));
    if (Number.isFinite(boost)) {
      return { field: spec.slice(0, at), boost };
    }
  }
  return { field: spec };
}

/** The filter verbs. */
export const Filter = {
  /** Every document. */
  all(): FilterJson {
    return { matchAll: {} };
  },
  /** No document. */
  none(): FilterJson {
    return { matchNone: {} };
  },
  /** The field equals `value` (a term query). */
  eq(field: string, value: JsonValue): FilterJson {
    return { term: { field, value } };
  },
  /** The field equals any of `values` (a terms query). */
  in(field: string, values: JsonValue[]): FilterJson {
    return { terms: { field, values } };
  },
  /** The field is within the bounds; an omitted bound is open. */
  range(field: string, bounds: Bounds): FilterJson {
    return { range: { field, ...definedBounds(bounds) } };
  },
  /** The field is present, whatever its value. */
  exists(field: string): FilterJson {
    return { exists: { field } };
  },
  /** The field is `null` or absent. */
  isNull(field: string): FilterJson {
    return { isNull: { field } };
  },
  /** The field is an empty string, list or object. */
  isEmpty(field: string): FilterJson {
    return { isEmpty: { field } };
  },
  /** The field starts with `value`. */
  prefix(field: string, value: string): FilterJson {
    return { prefix: { field, value } };
  },
  /** The field matches a `*`/`?` pattern. */
  wildcard(field: string, pattern: string): FilterJson {
    return { wildcard: { field, pattern } };
  },
  /** The field is within `fuzziness` edits of `value`. */
  fuzzy(field: string, value: string, edits?: Fuzziness): FilterJson {
    return { fuzzy: { field, value, ...(edits === undefined ? {} : { fuzziness: fuzziness(edits) }) } };
  },
  /** A full-text match on one field. */
  match(field: string, text: string, options: MatchOptions = {}): FilterJson {
    const match: JsonObject = { field, text };
    if (options.operator !== undefined) {
      match.operator = options.operator;
    }
    if (options.minimumShouldMatch !== undefined) {
      match.minimumShouldMatch = String(options.minimumShouldMatch);
    }
    if (options.fuzziness !== undefined) {
      match.fuzziness = fuzziness(options.fuzziness);
    }
    if (options.analyzer !== undefined) {
      match.analyzer = options.analyzer;
    }
    return { match };
  },
  /** The words of `text`, in order, at most `slop` positions apart. */
  phrase(field: string, text: string, slop?: number): FilterJson {
    return { matchPhrase: { field, text, ...(slop === undefined ? {} : { slop }) } };
  },
  /** A full-text match over several fields; `'title^2'` boosts one. */
  multiMatch(fields: string[], text: string, options: MultiMatchOptions = {}): FilterJson {
    const query: JsonObject = { fields: fields.map(multiMatchField), text };
    if (options.kind !== undefined) {
      query.kind = `MULTI_MATCH_KIND_${options.kind.toUpperCase()}`;
    }
    if (options.operator !== undefined) {
      query.operator = boolOperator(options.operator);
    }
    if (options.tieBreaker !== undefined) {
      query.tieBreaker = options.tieBreaker;
    }
    return { multiMatch: query };
  },
  /** The query-string syntax, over `fields` or the collection's defaults. */
  queryString(query: string, options: QueryStringOptions = {}): FilterJson {
    const out: JsonObject = { query };
    if (options.fields !== undefined && options.fields.length > 0) {
      out.defaultFields = options.fields;
    }
    if (options.operator !== undefined) {
      out.defaultOperator = boolOperator(options.operator);
    }
    return { queryString: out };
  },
  /** The documents with these ids, and no others. */
  ids(...ids: DocId[]): FilterJson {
    return { ids: ids.map(idValue) };
  },
  /** Every query matches (a bool `must`). */
  and(...queries: FilterJson[]): FilterJson {
    return { bool: { must: queries } };
  },
  /** At least one of the queries matches (a bool `should`). */
  or(...queries: FilterJson[]): FilterJson {
    return { bool: { should: queries } };
  },
  /** At least `minimumShouldMatch` of the queries match: `or` with an explicit
   * `minimum_should_match`. */
  anyOf(queries: FilterJson[], minimumShouldMatch: number | string): FilterJson {
    return { bool: { should: queries, minimumShouldMatch: String(minimumShouldMatch) } };
  },
  /** None of the queries match (a bool `must_not`). */
  not(...queries: FilterJson[]): FilterJson {
    return { bool: { mustNot: queries } };
  },
  /** Every query matches, without scoring (a bool `filter`). */
  filter(...queries: FilterJson[]): FilterJson {
    return { bool: { filter: queries } };
  },
  /** Multiplies the query's score by `boost`. */
  boost(query: FilterJson, boost: number): FilterJson {
    return { boost: { query, boost } };
  },
  /** Every match of `query` scores `score`. */
  constantScore(query: FilterJson, score: number): FilterJson {
    return { constantScore: { query, score } };
  },
} as const;

/** Decodes a filter into the generated `Query` message. */
export function toQuery(filter: FilterJson): Query {
  return fromJson(QuerySchema, filter);
}
