// The search builder (SDKP §2.4): a `loams.collection.v1.SearchRequest`, one
// verb at a time.
//
//   const request = Search.on('notes')
//     .text('refund', { fields: ['body'] })
//     .vector('embedding', [0.1, 0.2, 0.3, 0.4], { k: 10 })
//     .fuse(rrf(60))
//     .filter(Filter.eq('tags', 'policy'))
//     .highlight('body')
//     .select({ include: ['title'] })
//     .limit(5);
//   const answer = await loams.search.query(request.build());
//
// Each verb is pure: it returns a new builder and leaves the old one as it
// was, so a base query can be shared and specialised. `toJson()` is the
// request's proto3 JSON, which is how the builder corpus spells each case;
// `build()` decodes it into the generated message, refusing the requests the
// builder can tell are wrong (a weighted fusion with the wrong number of
// weights) before any network call.

import { fromJson, toJson, type JsonObject, type JsonValue } from '@bufbuild/protobuf';
import { ValueSchema, type Value } from '@bufbuild/protobuf/wkt';
import { Code } from '@connectrpc/connect';
import { SearchRequestSchema, type SearchRequest } from '@loams/proto/query';
import { ConsistencyToken } from '../runtime/consistency.js';
import { InvalidArgumentError } from '../runtime/errors.js';
import { Filter, type FilterJson } from './filter.js';

/** A fusion, as the proto3 JSON of `loams.collection.v1.Fusion`. */
export type FusionJson = JsonObject;

/** Reciprocal-rank fusion; `k` absent is the engine's 60. */
export function rrf(k?: number): FusionJson {
  return { rrf: k === undefined ? {} : { k } };
}

/** Distance-based score fusion, with the engine's parameters. */
export function dbsf(): FusionJson {
  return { dbsf: {} };
}

/** A weighted sum, one weight per retriever in the order they were added. */
export function weighted(...weights: number[]): FusionJson {
  return { weightedSum: { weights } };
}

/** The fusions, as one namespace. */
export const Fusion = { rrf, dbsf, weighted } as const;

/** One key of the sort, as the proto3 JSON of `SortKey`. */
export type SortJson = JsonObject;

type Order = 'asc' | 'desc';
const order = (value: Order) => (value === 'asc' ? 'SORT_ORDER_ASC' : 'SORT_ORDER_DESC');

/** The sort keys. The primary key ascending always breaks ties on the server,
 * but `queryAll` needs it spelled out to page. */
export const Sort = {
  /** By score; descending unless told otherwise. */
  score(direction?: Order): SortJson {
    return { score: direction === undefined ? {} : { order: order(direction) } };
  },
  /** By primary key; ascending unless told otherwise. */
  pk(direction?: Order): SortJson {
    return { pk: direction === undefined ? {} : { order: order(direction) } };
  },
  /** By a fast field. `missing` puts documents without it first or last. */
  field(name: string, direction?: Order, missing?: 'first' | 'last'): SortJson {
    const key: JsonObject = { field: name };
    if (direction !== undefined) {
      key.order = order(direction);
    }
    if (missing !== undefined) {
      key.missing = missing === 'first' ? 'MISSING_ORDER_FIRST' : 'MISSING_ORDER_LAST';
    }
    return { field: key };
  },
} as const;

/** The options of a text retriever. */
export interface TextOptions {
  /** One field is a `match`; several are a `multi_match`; none is a
   * `query_string` over the collection's default fields. */
  fields?: string[];
  /** How many candidates the retriever contributes. */
  k?: number;
}

/** A vector retriever's options: the candidate count and the ANN knobs. */
export interface VectorOptions {
  k?: number;
  exact?: boolean;
  nprobes?: number;
  refineFactor?: number;
  ef?: number;
  oversampling?: number;
  /** Overrides the schema's distance for this query. */
  distance?: string;
  /** A pre-filter on this retriever's candidates. */
  filter?: FilterJson;
}

/** A sparse retriever's options. */
export interface SparseOptions {
  k?: number;
  filter?: FilterJson;
}

/** What to return of each hit. */
export interface SelectOptions {
  /** `_source` paths to keep; absent keeps everything. */
  include?: string[];
  /** `_source` paths to drop. */
  exclude?: string[];
  /** `true` returns no `_source` at all. */
  noSource?: boolean;
  /** Vectors to return by name. */
  vectors?: string[];
  /** Fast fields to return by name. */
  fields?: string[];
}

/** A highlighted field's options. */
export interface HighlightOptions {
  pre?: string;
  post?: string;
  fragmentSize?: number;
  fragments?: number;
}

/** A graph expansion from the top hits (design §07 §5.2). */
export interface ExpandOptions {
  /** The source field holding the vertex key. */
  fromField?: string;
  seedLabel?: string;
  seeds?: number;
  hops?: number;
  direction?: 'out' | 'in' | 'both';
  edgeTypes?: string[];
  limitPerSeed?: number;
  limit?: number;
  output?: 'nested' | 'flat';
}

/** How fresh the read must be. A token, or its text, is `at_least`. */
export type ConsistencyLevel = 'strong' | 'eventual' | ConsistencyToken | string;

/** The proto3 JSON of a `loams.collection.v1.Consistency`. */
export function consistencyJson(level: ConsistencyLevel): JsonObject {
  if (level === 'strong') {
    return { freshness: 'FRESHNESS_STRONG' };
  }
  if (level === 'eventual') {
    return { freshness: 'FRESHNESS_EVENTUAL' };
  }
  const token = typeof level === 'string' ? ConsistencyToken.parse(level) : level;
  return { atLeast: token.toString() };
}

/** A hit's sort value, as `Hit.sort_values` answers it or as JSON. */
export type SortValue = JsonValue | Value;

function sortValueJson(value: SortValue): JsonValue {
  if (
    value !== null &&
    typeof value === 'object' &&
    !Array.isArray(value) &&
    (value as { $typeName?: string }).$typeName === 'google.protobuf.Value'
  ) {
    return toJson(ValueSchema, value as Value);
  }
  return value as JsonValue;
}

function textQuery(text: string | FilterJson, fields: string[] | undefined): FilterJson {
  if (typeof text !== 'string') {
    return text;
  }
  if (fields === undefined || fields.length === 0) {
    return Filter.queryString(text);
  }
  if (fields.length === 1) {
    return Filter.match(fields[0] as string, text);
  }
  return Filter.multiMatch(fields, text);
}

/** The search builder. */
export class Search {
  readonly #json: JsonObject;

  private constructor(json: JsonObject) {
    this.#json = json;
  }

  /** A search of `collection` (a name or an alias). */
  static on(collection: string, options: { namespace?: string } = {}): Search {
    return new Search({
      ...(options.namespace === undefined ? {} : { namespace: options.namespace }),
      collection,
    });
  }

  /** A builder over an existing request's proto3 JSON. */
  static fromJson(json: JsonObject): Search {
    return new Search(structuredClone(json));
  }

  #with(patch: JsonObject): Search {
    return new Search({ ...structuredClone(this.#json), ...patch });
  }

  #retrievers(): JsonValue[] {
    return (this.#json.retrievers as JsonValue[] | undefined) ?? [];
  }

  /** The namespace, when it is not the client's default. */
  namespace(namespace: string): Search {
    return this.#with({ namespace });
  }

  /** A text retriever. A string is a `match`/`multi_match`/`query_string`
   * by `fields`; a `Filter` (`match`, `multiMatch`, `phrase`…) is used as is. */
  text(text: string | FilterJson, options: TextOptions = {}): Search {
    const retriever: JsonObject = { query: textQuery(text, options.fields) };
    if (options.k !== undefined) {
      retriever.k = options.k;
    }
    return this.#with({ retrievers: [...this.#retrievers(), { text: retriever }] });
  }

  /** A dense-vector kNN retriever on `field`. */
  vector(field: string, values: readonly number[], options: VectorOptions = {}): Search {
    const retriever: JsonObject = { field, query: [...values] };
    if (options.k !== undefined) {
      retriever.k = options.k;
    }
    const params: JsonObject = {};
    for (const key of ['exact', 'nprobes', 'refineFactor', 'ef', 'oversampling', 'distance'] as const) {
      if (options[key] !== undefined) {
        params[key] = options[key] as JsonValue;
      }
    }
    if (Object.keys(params).length > 0) {
      retriever.params = params;
    }
    if (options.filter !== undefined) {
      retriever.filter = options.filter;
    }
    return this.#with({ retrievers: [...this.#retrievers(), { vector: retriever }] });
  }

  /** A sparse-vector retriever: `{indices, values}`, or a term-to-weight map
   * whose keys are the indices. */
  sparse(
    field: string,
    terms: { indices: number[]; values: number[] } | Record<number, number>,
    options: SparseOptions = {},
  ): Search {
    const query =
      'indices' in terms && Array.isArray(terms.indices)
        ? { indices: [...terms.indices], values: [...(terms as { values: number[] }).values] }
        : (() => {
            const entries = Object.entries(terms as Record<number, number>);
            return { indices: entries.map(([index]) => Number(index)), values: entries.map(([, v]) => v) };
          })();
    const retriever: JsonObject = { field, query };
    if (options.k !== undefined) {
      retriever.k = options.k;
    }
    if (options.filter !== undefined) {
      retriever.filter = options.filter;
    }
    return this.#with({ retrievers: [...this.#retrievers(), { sparse: retriever }] });
  }

  /** How the retrievers' ranked lists combine: `rrf()`, `dbsf()` or
   * `weighted(…)`. */
  fuse(fusion: FusionJson): Search {
    return this.#with({ fusion });
  }

  /** The filter every candidate must match. */
  filter(filter: FilterJson): Search {
    return this.#with({ filter });
  }

  /** The sort, replacing any earlier one. */
  sort(...keys: SortJson[]): Search {
    return this.#with({ sort: keys });
  }

  /** The window size; the engine's default is 10. */
  limit(limit: number): Search {
    return this.#with({ limit });
  }

  /** How many hits to skip before the window. */
  offset(offset: number): Search {
    return this.#with({ offset });
  }

  /** The page after the hit whose `sortValues` these are. */
  after(sortValues: readonly SortValue[]): Search {
    return this.#with({ searchAfter: sortValues.map(sortValueJson) });
  }

  /** Drops hits scoring below `score`. */
  minScore(score: number): Search {
    return this.#with({ scoreThreshold: score });
  }

  /** What to return of each hit. */
  select(options: SelectOptions): Search {
    const select: JsonObject = {};
    if (options.noSource === true) {
      select.source = 'none';
    } else if (options.include !== undefined || options.exclude !== undefined) {
      const source: JsonObject = {};
      if (options.include !== undefined) {
        source.include = [...options.include];
      }
      if (options.exclude !== undefined) {
        source.exclude = [...options.exclude];
      }
      select.source = source;
    }
    if (options.vectors !== undefined) {
      select.vectors = [...options.vectors];
    }
    if (options.fields !== undefined) {
      select.fields = [...options.fields];
    }
    return this.#with({ select });
  }

  /** Highlights `field` in each hit; call once per field. */
  highlight(field: string, options: HighlightOptions = {}): Search {
    const entry: JsonObject = { field };
    if (options.pre !== undefined) {
      entry.preTag = options.pre;
    }
    if (options.post !== undefined) {
      entry.postTag = options.post;
    }
    if (options.fragmentSize !== undefined) {
      entry.fragmentSize = options.fragmentSize;
    }
    if (options.fragments !== undefined) {
      entry.numberOfFragments = options.fragments;
    }
    const existing = ((this.#json.highlight as JsonObject | undefined)?.fields as JsonValue[] | undefined) ?? [];
    return this.#with({ highlight: { fields: [...existing, entry] } });
  }

  /** Groups the hits by `field`: `size` hits per group, `limit` groups. */
  groupBy(field: string, options: { size?: number; limit?: number } = {}): Search {
    const group: JsonObject = { field };
    if (options.size !== undefined) {
      group.groupSize = options.size;
    }
    if (options.limit !== undefined) {
      group.limit = options.limit;
    }
    return this.#with({ groupBy: group });
  }

  /** Walks `graph` from the top hits and returns their neighbours. */
  expand(graph: string, options: ExpandOptions = {}): Search {
    const spec: JsonObject = { graph };
    const copy = ['fromField', 'seedLabel', 'seeds', 'hops', 'limitPerSeed', 'limit'] as const;
    for (const key of copy) {
      if (options[key] !== undefined) {
        spec[key] = options[key] as JsonValue;
      }
    }
    if (options.direction !== undefined) {
      spec.direction = `EXPAND_DIRECTION_${options.direction.toUpperCase()}`;
    }
    if (options.edgeTypes !== undefined) {
      spec.edgeTypes = [...options.edgeTypes];
    }
    if (options.output !== undefined) {
      spec.output = `EXPAND_OUTPUT_${options.output.toUpperCase()}`;
    }
    return this.#with({ expandSpec: spec });
  }

  /** How fresh the read must be. */
  consistency(level: ConsistencyLevel): Search {
    return this.#with({ consistency: consistencyJson(level) });
  }

  /** The collection searched. */
  get collection(): string {
    return this.#json.collection as string;
  }

  /** Whether the sort ends in a primary-key tiebreaker, which `queryAll`
   * needs to page without skipping or repeating a hit. */
  get hasTiebreak(): boolean {
    const sort = this.#json.sort as JsonObject[] | undefined;
    const last = sort?.[sort.length - 1];
    return last !== undefined && 'pk' in last;
  }

  /** The request's proto3 JSON. */
  toJson(): JsonObject {
    return structuredClone(this.#json);
  }

  /** The request as the generated message, checked. */
  build(): SearchRequest {
    const weights = ((this.#json.fusion as JsonObject | undefined)?.weightedSum as JsonObject | undefined)
      ?.weights as number[] | undefined;
    const retrievers = this.#retrievers().length;
    if (weights !== undefined && weights.length !== retrievers) {
      throw new InvalidArgumentError(
        `a weighted fusion needs one weight per retriever: ${weights.length} weights for ${retrievers} retrievers`,
        {
          code: Code.InvalidArgument,
          metadata: { field: 'fusion.weighted_sum.weights' },
        },
      );
    }
    return fromJson(SearchRequestSchema, this.#json);
  }
}
