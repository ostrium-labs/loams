// @vitest-environment node
import { describe, expect, it } from 'vite-plus/test';
import { schemaFields } from '../src/search/kql.js';
import { applySuggestion, type SuggestInput, suggest } from '../src/search/suggest.js';

const FIELDS = schemaFields({
  fields: [
    { name: 'title', kind: { text: {} }, indexed: true },
    { name: 'type', kind: 'keyword', indexed: true, fast: true },
    { name: 'tenant', kind: 'keyword', indexed: true, fast: true },
    { name: 'body_chars', kind: 'i64', indexed: true, fast: true },
    { name: 'public', kind: 'bool', indexed: true, fast: true },
    { name: 'created_at', kind: 'date', indexed: true, fast: true },
  ],
  vectors: [{ name: 'embedding', dim: 3 }],
});

const VALUES: Record<string, string[]> = {
  type: ['PushEvent', 'PullRequestEvent', 'CreateEvent', 'Watch Event'],
};

/** `|` marks the cursor. */
function at(q: string, extra: Partial<SuggestInput> = {}) {
  const cursor = q.indexOf('|');
  // Exactly one marker: dropping it by index keeps any other `|` in the query.
  const text = cursor < 0 ? q : q.slice(0, cursor) + q.slice(cursor + 1);
  return suggest({
    text,
    cursor,
    fields: FIELDS,
    values: (field, prefix) =>
      VALUES[field]?.filter((v) => v.startsWith(prefix)).map((value) => ({ value, count: 3 })),
    ...extra,
  });
}
const labels = (r: ReturnType<typeof suggest>) => r.items.map((i) => i.label);

describe('suggest', () => {
  it('empty: recent queries, then fields (no vectors)', () => {
    const r = at('|', { recent: ['type:PushEvent'] });
    expect(r.context.kind).toBe('empty');
    expect(labels(r)).toEqual([
      'type:PushEvent',
      'title',
      'type',
      'tenant',
      'body_chars',
      'public',
      'created_at',
      '_id',
    ]);
    expect(r.items[1]?.detail).toBe('text');
  });

  it('a partial word: fields by prefix, then by substring', () => {
    const r = at('t|');
    expect(r.context).toEqual({ kind: 'field', prefix: 't' });
    expect(labels(r)).toEqual(['title', 'type', 'tenant', 'created_at']);
    const r2 = at('chars|');
    expect(labels(r2)).toEqual(['body_chars']);
  });

  it('the cursor in the middle of a word replaces the whole word', () => {
    const r = at('ty|pe');
    const s = r.items.find((i) => i.label === 'type');
    expect(s && [s.from, s.to]).toEqual([0, 4]);
  });

  it('after a field name and a space: operators for its type', () => {
    const r = at('body_chars |');
    expect(r.context).toEqual({ kind: 'operator', field: 'body_chars' });
    expect(labels(r)).toEqual([
      'body_chars :',
      'body_chars >=',
      'body_chars >',
      'body_chars <=',
      'body_chars <',
      'body_chars :*',
    ]);
    const text = at('title |');
    expect(labels(text)).toEqual(['title :', 'title :*']);
  });

  it('picking an operator joins it to the field', () => {
    const r = at('type |');
    const colon = r.items[0];
    expect(colon && applySuggestion('type ', colon)).toEqual({ text: 'type:', cursor: 5 });
    const ge = at('body_chars |').items[1];
    expect(ge && applySuggestion('body_chars ', ge)).toEqual({
      text: 'body_chars >= ',
      cursor: 14,
    });
  });

  it('after field: values from the cache, quoted when needed', () => {
    const r = at('type:|');
    expect(r.context).toEqual({ kind: 'value', field: 'type', prefix: '', type: 'keyword' });
    expect(labels(r)).toEqual(['PushEvent', 'PullRequestEvent', 'CreateEvent', 'Watch Event']);
    expect(r.items[3]?.insert).toBe('"Watch Event" ');
    expect(r.items[0]?.detail).toBe('3');
  });

  it('a partial value narrows by prefix and replaces the value', () => {
    const r = at('type:Pu|');
    expect(r.context).toMatchObject({ kind: 'value', prefix: 'Pu' });
    expect(labels(r)).toEqual(['PushEvent', 'PullRequestEvent']);
    const first = r.items[0];
    expect(first && applySuggestion('type:Pu', first)).toEqual({
      text: 'type:PushEvent ',
      cursor: 15,
    });
  });

  it('a partial quoted value strips the quote from the prefix', () => {
    expect(at('type:"Wa|').context).toMatchObject({ kind: 'value', prefix: 'Wa' });
  });

  it('values inside a list', () => {
    expect(at('type:(PushEvent OR |').context).toMatchObject({ kind: 'value', field: 'type' });
    expect(at('type:(Cr|').context).toMatchObject({ kind: 'value', prefix: 'Cr' });
  });

  it('booleans suggest true and false', () => {
    expect(labels(at('public:|'))).toEqual(['true', 'false']);
    expect(labels(at('public:f|'))).toEqual(['false']);
  });

  it('typed fields without values give a hint that cannot be picked', () => {
    const r = at('created_at >= |');
    expect(r.items).toHaveLength(1);
    expect(r.items[0]?.kind).toBe('hint');
    expect(r.items[0]?.insert).toBeUndefined();
  });

  it('after a complete clause: AND, OR, NOT, then fields', () => {
    const r = at('type:PushEvent |');
    expect(r.context.kind).toBe('after-clause');
    expect(labels(r).slice(0, 4)).toEqual(['AND', 'OR', 'NOT', 'title']);
    expect(at('(a:b) |').context.kind).toBe('after-clause');
  });

  it('typing a conjunction after a clause offers it', () => {
    expect(labels(at('type:X A|')).slice(-1)).toEqual(['AND']);
    expect(labels(at('type:X O|'))).toContain('OR');
  });

  it('after AND or (: fields', () => {
    expect(at('type:X AND |').context).toEqual({ kind: 'field', prefix: '' });
    expect(labels(at('(|'))[0]).toBe('title');
  });

  it('applySuggestion does not double a space', () => {
    expect(
      applySuggestion('ty rest', { kind: 'field', label: 'type', insert: 'type ', from: 0, to: 2 }),
    ).toEqual({ text: 'type rest', cursor: 5 });
  });

  it('_id is offered and hinted', () => {
    expect(labels(at('_i|'))).toEqual(['_id']);
    expect(at('_id:|').items[0]?.label).toContain('document id');
  });

  it('limits the list', () => {
    expect(at('|', { limit: 2 }).items).toHaveLength(2);
  });
});
