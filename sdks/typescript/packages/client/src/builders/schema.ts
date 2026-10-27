// The schema builder (SDKP §2.6): `CreateCollectionRequest.schema`, a `Struct`
// in the REST route's schema JSON, without writing the JSON by hand.
//
//   const schema = new Schema()
//     .text('title').text('body').keyword('tags').i64('n')
//     .vector('embedding', 4);
//   await loams.collections.create({ name: 'notes', schema: schema.toJson() });
//
// The output is the shape `crates/loams/tests/connect_collections.rs`
// `kb_schema()` sends: every field spelled out (`name`, `source_path`, `kind`,
// `indexed`, `fast`), the vectors and sparse vectors as lists, and `dynamic`
// and `max_fields` always present, so what the builder sends is what the
// server stores and nothing rides on a server default.

import type { JsonObject } from '@bufbuild/protobuf';

/** What every field takes. */
export interface FieldOptions {
  /** Whether the field is in the inverted index. Default `true`. */
  indexed?: boolean;
  /** Whether the field has a columnar copy for sorting, ranges and
   * aggregations. Default `false` for text and `true` for every other kind. */
  fast?: boolean;
  /** Where the value comes from in `_source`. Default: the field's name. */
  sourcePath?: string;
}

/** A text field's options. */
export interface TextOptions extends FieldOptions {
  /** `standard` (the default), `english`, `simple`, `whitespace` or `keyword`. */
  analyzer?: string;
  /** Whether positions are indexed, which phrase queries need. Default `true`. */
  positions?: boolean;
}

/** A dense vector's distance function. */
export type Distance = 'cosine' | 'dot' | 'euclid' | 'manhattan';

/** What a write does with a `_source` path no field maps. */
export type DynamicMapping = 'ignore' | 'strict' | 'map';

/** The schema builder. Each verb appends and returns the builder. */
export class Schema {
  readonly #fields: JsonObject[] = [];
  readonly #vectors: JsonObject[] = [];
  readonly #sparse: JsonObject[] = [];
  #dynamic: DynamicMapping = 'ignore';
  #maxFields = 1000;

  #field(name: string, kind: JsonObject[string], fastByDefault: boolean, options: FieldOptions) {
    this.#fields.push({
      name,
      source_path: options.sourcePath ?? name,
      kind,
      indexed: options.indexed ?? true,
      fast: options.fast ?? fastByDefault,
    });
    return this;
  }

  /** A full-text field. */
  text(name: string, options: TextOptions = {}): this {
    return this.#field(
      name,
      { text: { analyzer: options.analyzer ?? 'standard', positions: options.positions ?? true } },
      false,
      options,
    );
  }

  /** An exact-match string field. */
  keyword(name: string, options: FieldOptions = {}): this {
    return this.#field(name, 'keyword', true, options);
  }

  /** A signed 64-bit integer field. */
  i64(name: string, options: FieldOptions = {}): this {
    return this.#field(name, 'i64', true, options);
  }

  /** A 64-bit float field. */
  f64(name: string, options: FieldOptions = {}): this {
    return this.#field(name, 'f64', true, options);
  }

  /** A boolean field. */
  bool(name: string, options: FieldOptions = {}): this {
    return this.#field(name, 'bool', true, options);
  }

  /** An RFC 3339 timestamp field (the schema's `date` kind). */
  timestamp(name: string, options: FieldOptions = {}): this {
    return this.#field(name, 'date', true, options);
  }

  /** A UUID field. */
  uuid(name: string, options: FieldOptions = {}): this {
    return this.#field(name, 'uuid', true, options);
  }

  /** A named dense vector of `dim` dimensions. */
  vector(name: string, dim: number, distance: Distance = 'cosine'): this {
    if (!Number.isInteger(dim) || dim <= 0) {
      throw new RangeError(`a vector's dim must be a positive integer, not ${dim}`);
    }
    this.#vectors.push({ name, dim, distance });
    return this;
  }

  /** A named sparse vector; `idf` reweights the query by IDF at search time. */
  sparseVector(name: string, modifier?: 'none' | 'idf'): this {
    this.#sparse.push(modifier === undefined ? { name } : { name, modifier });
    return this;
  }

  /** What a write does with unmapped `_source` paths. Default `ignore`. */
  dynamic(mode: DynamicMapping): this {
    this.#dynamic = mode;
    return this;
  }

  /** The most fields, vectors and sparse vectors together. Default 1000. */
  maxFields(n: number): this {
    this.#maxFields = n;
    return this;
  }

  /** The schema `Struct`, as `CreateCollectionRequest.schema` takes it. */
  toJson(): JsonObject {
    return {
      fields: this.#fields.map((field) => ({ ...field })),
      vectors: this.#vectors.map((vector) => ({ ...vector })),
      sparse_vectors: this.#sparse.map((sparse) => ({ ...sparse })),
      dynamic: this.#dynamic,
      max_fields: this.#maxFields,
    };
  }
}
