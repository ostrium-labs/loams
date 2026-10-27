// SDKP's `typescript_builders_match_corpus`.
//
// Every case in `sdks/fixtures/builders/` is driven through the public
// builders — `Schema`, `Filter`, `Search` — from its neutral spec, and the
// output is compared with the case's `expect` after both are decoded into the
// named message, so key order and number spelling do not matter and a field
// the message does not have does. The notation is in that directory's README.

import { fromJson, toJson } from '@bufbuild/protobuf';
import { QuerySchema, SearchRequestSchema } from '@loams/proto/query';
import { describe, expect, it } from 'vite-plus/test';
import { Search, weighted } from '../src/builders/search.js';
import { InvalidArgumentError } from '../src/runtime/errors.js';
import { args, cases, filter, schema, search, type Call } from './helpers/builders.js';

describe('typescript_builders_match_corpus', () => {
  it('has the corpus', () => {
    expect(cases.map((c) => c.name)).toEqual(
      expect.arrayContaining([
        'schema_notes',
        'filter_term',
        'filter_bool_nested',
        'filter_range_open',
        'filter_ids',
        'search_text_filter_highlight_select',
        'search_vector_ann_params',
        'search_hybrid_rrf',
        'search_hybrid_dbsf',
        'search_hybrid_weighted',
        'search_after_page',
        'search_expand',
      ]),
    );
  });

  it.each(cases)('$name', (c) => {
    switch (c.builder) {
      case 'schema':
        expect(schema(c.spec as Call[])).toEqual(c.expect);
        return;
      case 'filter': {
        const built = filter((c.spec as Call).op, args(c.spec as Call));
        expect(toJson(QuerySchema, fromJson(QuerySchema, built))).toEqual(
          toJson(QuerySchema, fromJson(QuerySchema, c.expect)),
        );
        return;
      }
      case 'search': {
        const built = search(c.spec as { on: string; calls: Call[] }).build();
        expect(toJson(SearchRequestSchema, built)).toEqual(
          toJson(SearchRequestSchema, fromJson(SearchRequestSchema, c.expect)),
        );
        return;
      }
    }
  });

  it('refuses a weighted fusion with the wrong number of weights before any call', () => {
    const builder = Search.on('notes')
      .text('refund', { fields: ['body'] })
      .vector('embedding', [1, 0, 0, 0])
      .fuse(weighted(1));
    expect(() => builder.build()).toThrow(InvalidArgumentError);
    try {
      builder.build();
    } catch (error) {
      expect((error as InvalidArgumentError).metadata.field).toBe('fusion.weighted_sum.weights');
    }
  });

  it('is pure: a verb leaves the builder it was called on unchanged', () => {
    const base = Search.on('notes').text('refund', { fields: ['body'] });
    const limited = base.limit(2);
    expect(base.toJson().limit).toBeUndefined();
    expect(limited.toJson().limit).toBe(2);
  });
});
