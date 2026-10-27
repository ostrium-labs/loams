// @vitest-environment node
import { describe, expect, it } from 'vite-plus/test';
import {
  closest,
  compile,
  compileQuery,
  type FieldInfo,
  lex,
  parse,
  quoteValue,
  schemaFields,
} from '../src/search/kql.js';

const SCHEMA = {
  fields: [
    { name: 'title', kind: { text: { analyzer: 'standard' } }, indexed: true, fast: false },
    { name: 'body', kind: { text: { analyzer: 'standard' } }, indexed: true, fast: false },
    { name: 'type', kind: 'keyword', indexed: true, fast: true },
    { name: 'body_chars', kind: 'i64', indexed: true, fast: true },
    { name: 'ratio', kind: 'f64', indexed: true, fast: true },
    { name: 'public', kind: 'bool', indexed: true, fast: true },
    { name: 'created_at', kind: 'date', indexed: true, fast: true },
    { name: 'payload', kind: 'json', indexed: true, fast: false },
    { name: 'raw', kind: 'keyword', indexed: false, fast: false },
  ],
  vectors: [{ name: 'embedding', dim: 3, distance: 'cosine' }],
};
const FIELDS: FieldInfo[] = schemaFields(SCHEMA);

function ok(q: string, opts?: { scored?: boolean }) {
  const r = compileQuery(q, FIELDS, opts);
  if (!r.ok) throw new Error(`${q}: ${r.error.message} @${r.error.from}-${r.error.to}`);
  return r.compiled;
}
function err(q: string) {
  const r = compileQuery(q, FIELDS);
  if (r.ok) throw new Error(`${q} compiled`);
  return r.error;
}

describe('schemaFields', () => {
  it('reads kinds, fast flags and vectors', () => {
    expect(FIELDS.map((f) => `${f.name}:${f.type}`)).toEqual([
      'title:text',
      'body:text',
      'type:keyword',
      'body_chars:i64',
      'ratio:f64',
      'public:bool',
      'created_at:date',
      'payload:json',
      'raw:keyword',
      'embedding:vector',
    ]);
    expect(FIELDS.find((f) => f.name === 'type')?.fast).toBe(true);
    expect(FIELDS.find((f) => f.name === 'embedding')?.dim).toBe(3);
  });
});

describe('lex', () => {
  it('reads a time after a colon as one value', () => {
    const t = lex('created_at>=2024-01-15T15:00:00Z').filter((x) => x.kind !== 'ws');
    expect(t.map((x) => x.kind)).toEqual(['word', 'cmp', 'word']);
    expect(t[2]?.value).toBe('2024-01-15T15:00:00Z');
  });
  it('marks an open quote', () => {
    const t = lex('"french riv');
    expect(t[0]?.open).toBe(true);
    expect(t[0]?.value).toBe('french riv');
  });
  it('keeps escapes in quotes', () => {
    expect(lex('"a \\"b\\""')[0]?.value).toBe('a "b"');
  });
});

describe('parse', () => {
  it('merges adjacent words into one free-text query', () => {
    const r = parse('french river');
    expect(r.ok && r.ast).toMatchObject({ kind: 'text', text: 'french river', phrase: false });
  });
  it('keeps an explicit AND between words', () => {
    const r = parse('french AND river');
    expect(r.ok && r.ast?.kind).toBe('and');
  });
  it('gives OR lower precedence than AND', () => {
    const r = parse('a:1 b:2 OR c:3');
    expect(r.ok && r.ast?.kind).toBe('or');
  });
  it('parses value lists', () => {
    const r = parse('type:(ForkEvent OR "Watch Event")');
    expect(r.ok && r.ast).toMatchObject({
      kind: 'field',
      field: 'type',
      values: [{ text: 'ForkEvent' }, { text: 'Watch Event', quoted: true }],
    });
  });
  it('treats an empty query as everything', () => {
    expect(parse('   ')).toEqual({ ok: true, ast: undefined });
  });

  it.each([
    ['type:', 'needs a value', 4, 5],
    ['(a', 'is not closed', 0, 1],
    ['a)', 'no matching', 1, 2],
    ['"open', 'quote is not closed', 0, 1],
    ['a AND', 'AND needs a clause', 2, 5],
    ['a OR', 'OR needs a clause', 2, 4],
    ['NOT', 'NOT needs a clause', 0, 3],
    ['type:(a AND b)', 'one value at a time', 8, 11],
    ['()', 'Empty parentheses', 0, 2],
    ['body_chars >', 'needs a value', 11, 12],
  ])('%s is an error at its span', (q, msg, from, to) => {
    const r = parse(q);
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.message).toContain(msg);
      expect([r.error.from, r.error.to]).toEqual([from, to]);
    }
  });
});

describe('compile', () => {
  it('free text is a scored match over every text field', () => {
    const c = ok('french river');
    expect(c.text).toEqual({
      bool: {
        should: [
          { match: { field: 'title', text: 'french river' } },
          { match: { field: 'body', text: 'french river' } },
        ],
      },
    });
    expect(c.filter).toBeUndefined();
    expect(c.terms).toEqual(['french river']);
    expect(c.highlightFields).toEqual(['title', 'body']);
  });

  it('a phrase is match_phrase', () => {
    expect(ok('"french river"').text).toMatchObject({
      bool: { should: [{ matchPhrase: { field: 'title', text: 'french river' } }, {}] },
    });
  });

  it('splits scored text from filters', () => {
    const c = ok('river type:PushEvent body_chars >= 200');
    expect(c.text).toMatchObject({ bool: { should: [{ match: { text: 'river' } }, {}] } });
    expect(c.filter).toEqual({
      bool: {
        must: [
          { term: { field: 'type', value: 'PushEvent' } },
          { range: { field: 'body_chars', gte: 200 } },
        ],
      },
    });
  });

  it('a text field clause is scored and highlighted on that field', () => {
    const c = ok('title:river');
    expect(c.text).toEqual({ match: { field: 'title', text: 'river' } });
    expect(c.highlightFields).toEqual(['title']);
  });

  it('in vector mode the words filter instead of ranking', () => {
    const c = ok('river type:X', { scored: false });
    expect(c.text).toBeUndefined();
    expect(c.filter).toMatchObject({
      bool: { must: [{ bool: { should: [{}, {}] } }, { term: {} }] },
    });
  });

  it('free text under OR with a field clause becomes a filter', () => {
    const c = ok('river OR type:X');
    expect(c.text).toBeUndefined();
    expect(c.filter).toMatchObject({
      bool: { should: [{ bool: {} }, { term: { field: 'type' } }] },
    });
  });

  it('NOT is must_not', () => {
    expect(ok('NOT type:PushEvent').filter).toEqual({
      bool: { mustNot: [{ term: { field: 'type', value: 'PushEvent' } }] },
    });
  });

  it('keyword wildcards: prefix, wildcard and exists', () => {
    expect(ok('type:Push*').filter).toEqual({ prefix: { field: 'type', value: 'Push' } });
    expect(ok('type:*kube*').filter).toEqual({ wildcard: { field: 'type', pattern: '*kube*' } });
    expect(ok('type:*').filter).toEqual({ exists: { field: 'type' } });
    // Quoted: the star is literal.
    expect(ok('type:"Push*"').filter).toEqual({ term: { field: 'type', value: 'Push*' } });
  });

  it('value lists are terms', () => {
    expect(ok('type:(A OR B)').filter).toEqual({ terms: { field: 'type', values: ['A', 'B'] } });
    expect(ok('body_chars:(1 OR 2)').filter).toEqual({
      terms: { field: 'body_chars', values: [1, 2] },
    });
  });

  it('typed values', () => {
    expect(ok('body_chars:199').filter).toEqual({ term: { field: 'body_chars', value: 199 } });
    expect(ok('ratio < 0.5').filter).toEqual({ range: { field: 'ratio', lt: 0.5 } });
    expect(ok('public:TRUE').filter).toEqual({ term: { field: 'public', value: true } });
    expect(ok('created_at:2024-01-15').filter).toEqual({
      range: { field: 'created_at', gte: '2024-01-15T00:00:00Z', lt: '2024-01-16T00:00:00Z' },
    });
    expect(ok('created_at > 2024-01-15T15:30:00Z').filter).toEqual({
      range: { field: 'created_at', gt: '2024-01-15T15:30:00Z' },
    });
    expect(ok('payload.user.id:42').filter).toEqual({
      term: { field: 'payload.user.id', value: '42' },
    });
  });

  it.each([
    ['tpye:Push', 'Did you mean type?', 0, 4],
    ['nothing_like_it:1', 'no field "nothing_like_it"', 0, 15],
    ['body_chars:abc', 'not a number', 11, 14],
    ['body_chars:1.5', 'whole numbers', 11, 14],
    ['public:maybe', 'true or false', 7, 12],
    ['created_at:yesterday', 'not a date', 11, 20],
    ['title > a', 'ranges work on', 0, 9],
    ['title:riv*', 'Wildcards work on keyword', 6, 10],
    ['embedding:1', 'vector field', 0, 9],
    ['raw:x', 'not indexed', 0, 3],
  ])('%s is a typed error at its span', (q, msg, from, to) => {
    const e = err(q);
    expect(e.message).toContain(msg);
    expect([e.from, e.to]).toEqual([from, to]);
  });

  it('_id is the document id: numbers are uint ids, the rest strings', () => {
    expect(ok('_id:42').filter).toEqual({ ids: [42] });
    expect(ok('_id:(1 OR abc OR "7")').filter).toEqual({ ids: [1, 'abc', '7'] });
    expect(ok('NOT _id:3').filter).toEqual({ bool: { mustNot: [{ ids: [3] }] } });
    expect(err('_id > 3').message).toContain('exact ids');
    expect(err('_id:4*').message).toContain('not wildcards');
  });

  it('free text with no text fields is an error', () => {
    const fields = FIELDS.filter((f) => f.type !== 'text');
    const parsed = parse('hello');
    expect(() => parsed.ok && compile(parsed.ast, fields)).toThrow(/no text fields/);
  });
});

describe('helpers', () => {
  it('quoteValue quotes only when needed', () => {
    expect(quoteValue('PushEvent')).toBe('PushEvent');
    expect(quoteValue('a/b-c.d')).toBe('a/b-c.d');
    expect(quoteValue('two words')).toBe('"two words"');
    expect(quoteValue('or')).toBe('"or"');
    expect(quoteValue('say "hi"')).toBe('"say \\"hi\\""');
    expect(quoteValue('')).toBe('""');
  });
  it('closest finds near names only', () => {
    expect(closest('tpye', ['type', 'title'])).toBe('type');
    expect(closest('zzzzzz', ['type', 'title'])).toBeUndefined();
  });
});
