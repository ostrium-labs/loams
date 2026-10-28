// @vitest-environment node
import { describe, expect, it } from 'vite-plus/test';
import { schemaFields } from '../src/search/kql.js';
import {
  buildRequest,
  defaultState,
  fromParams,
  HIGHLIGHT_POST,
  HIGHLIGHT_PRE,
  highlightRuns,
  markTerms,
  parseVector,
  toParams,
} from '../src/search/request.js';

const FIELDS = schemaFields({
  fields: [
    { name: 'body', kind: { text: {} }, indexed: true },
    { name: 'letter', kind: 'keyword', indexed: true, fast: true },
  ],
  vectors: [{ name: 'embedding', dim: 3, distance: 'cosine' }],
});
const VEC = [0.1, 0.2, 0.3];

function build(patch: Partial<ReturnType<typeof defaultState>>, vec?: number[]) {
  const r = buildRequest({ ...defaultState(FIELDS), ...patch }, FIELDS, vec);
  if (!r.ok) throw new Error(r.error.message);
  return r.request;
}

describe('buildRequest', () => {
  it('hybrid with a vector fuses BM25 and the vector by RRF, and asks for each signal', () => {
    const r = build({ q: 'river letter:F', rrfK: 30 }, VEC);
    expect(r.ir.retrievers).toEqual([
      { text: { query: { match: { field: 'body', text: 'river' } }, k: 20 } },
      { vector: { field: 'embedding', query: VEC, k: 20 } },
    ]);
    expect(r.ir.fusion).toEqual({ rrf: { k: 30 } });
    expect(r.ir.filter).toEqual({ term: { field: 'letter', value: 'F' } });
    expect(r.ir.trackTotalHits).toBe('exact');
    expect(r.signals?.text.retrievers).toHaveLength(1);
    expect(r.signals?.vector.filter).toEqual(r.ir.filter);
    expect(r.plan).toBe('BM25 + vector, RRF k=30');
    expect(r.ir.highlight).toMatchObject({
      fields: [{ field: 'body', preTag: HIGHLIGHT_PRE, postTag: HIGHLIGHT_POST }],
    });
  });

  it('weighted sum uses the text weight', () => {
    const r = build({ q: 'river', fusion: 'weighted', textWeight: 0.3 }, VEC);
    expect(r.ir.fusion).toEqual({ weightedSum: { weights: [0.3, 0.7] } });
  });

  it('hybrid without a vector is a text search, and says so', () => {
    const r = build({ q: 'river' });
    expect(r.ir.retrievers).toHaveLength(1);
    expect(r.ir.fusion).toBeUndefined();
    expect(r.notes.join(' ')).toContain('Hybrid without a vector');
  });

  it('vector mode filters by the words instead of ranking', () => {
    const r = build({ q: 'river', mode: 'vector' }, VEC);
    expect(r.ir.retrievers).toEqual([{ vector: { field: 'embedding', query: VEC, k: 20 } }]);
    expect(r.ir.filter).toEqual({ match: { field: 'body', text: 'river' } });
    expect(r.ir.highlight).toBeUndefined();
  });

  it('an empty query with no vector browses by filter', () => {
    const r = build({ pills: [{ id: 'a', q: 'letter:F', negate: true, disabled: false }] });
    expect(r.ir.retrievers).toBeUndefined();
    expect(r.ir.filter).toEqual({ bool: { mustNot: [{ term: { field: 'letter', value: 'F' } }] } });
    expect(r.plan).toBe('Filter only, in id order');
  });

  it('pills are ANDed and a disabled pill is left out', () => {
    const r = build({
      q: 'letter:A',
      pills: [
        { id: 'a', q: 'letter:B', negate: false, disabled: false },
        { id: 'b', q: 'letter:C', negate: false, disabled: true },
      ],
    });
    expect(r.ir.filter).toEqual({
      bool: {
        must: [
          { term: { field: 'letter', value: 'A' } },
          { term: { field: 'letter', value: 'B' } },
        ],
      },
    });
  });

  it('names the pill that does not compile', () => {
    const r = buildRequest(
      {
        ...defaultState(FIELDS),
        pills: [{ id: 'p9', q: 'nope:1', negate: false, disabled: false }],
      },
      FIELDS,
    );
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.source).toBe('p9');
  });
});

describe('URL state', () => {
  it('round-trips through the query string, defaults left out', () => {
    const s = {
      ...defaultState(FIELDS),
      q: 'river letter:F',
      pills: [{ id: 'x', q: 'letter:B', negate: true, disabled: false }],
      k: 50,
      fusion: 'weighted' as const,
      textWeight: 0.25,
      similarTo: '88177',
      view: 'table' as const,
      columns: ['body', 'letter'],
    };
    const qs = toParams(s, FIELDS);
    expect(qs).not.toContain('mode=');
    const back = fromParams(Object.fromEntries(new URLSearchParams(qs)), FIELDS);
    expect({ ...back, pills: back.pills.map(({ id: _, ...p }) => p) }).toEqual({
      ...s,
      pills: [{ q: 'letter:B', negate: true, disabled: false }],
    });
    expect(toParams(defaultState(FIELDS), FIELDS)).toBe('');
  });

  it('ignores bad values', () => {
    const s = fromParams({ mode: 'magic', k: '-3', f: '{broken', vf: 'nope', tw: '7' }, FIELDS);
    expect(s.mode).toBe('hybrid');
    expect(s.k).toBe(20);
    expect(s.pills).toEqual([]);
    expect(s.vectorField).toBe('embedding');
    expect(s.textWeight).toBe(0.5);
  });

  it('a collection without vectors is text only', () => {
    const fields = FIELDS.filter((f) => f.type !== 'vector');
    expect(fromParams({ mode: 'vector' }, fields).mode).toBe('text');
  });
});

describe('vectors and highlights', () => {
  it('parseVector checks shape and dimension', () => {
    expect(parseVector('[1, 2, 3]', 3)).toEqual([1, 2, 3]);
    expect(() => parseVector('[1, 2]', 3)).toThrow(/takes 3/);
    expect(() => parseVector('[1, "a"]')).toThrow(/JSON array/);
    expect(() => parseVector('nope')).toThrow(/JSON array/);
  });

  it('highlightRuns splits the engine fragment on its markers', () => {
    expect(highlightRuns(`a ${HIGHLIGHT_PRE}River${HIGHLIGHT_POST} b`)).toEqual([
      { text: 'a ', hit: false },
      { text: 'River', hit: true },
      { text: ' b', hit: false },
    ]);
  });

  it('markTerms marks whole words, case-insensitively, never markup', () => {
    expect(markTerms('The river Rivers <b>', ['RIVER', '<b>'])).toEqual([
      { text: 'The ', hit: false },
      { text: 'river', hit: true },
      { text: ' Rivers <b>', hit: false },
    ]);
  });
});
