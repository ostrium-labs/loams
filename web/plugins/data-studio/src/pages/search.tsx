import type { JsonObject } from '@bufbuild/protobuf';
import { Button, Card, Checkbox, Empty, Field, Input, Select, Table, Textarea } from '@loams/ui';
import { type FormEvent, useState } from 'react';
import { type DataClient, type SearchHit, vectorFields } from '../client.js';
import { ErrorNotice, errorText } from '../shared.js';

export interface SearchForm {
  text: string;
  vector: string;
  vectorField: string;
  filter: string;
  limit: number;
  hybrid: boolean;
}

/** Builds the SearchRequest IR (proto JSON) from the form; throws a readable error. */
export function buildSearchIr(f: SearchForm): JsonObject {
  const retrievers: JsonObject[] = [];
  let vector: number[] | undefined;
  if (f.vector.trim()) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(f.vector);
    } catch {
      throw new Error('The vector must be a JSON array of numbers, like [0.1, 0.2].');
    }
    if (
      !Array.isArray(parsed) ||
      parsed.length === 0 ||
      parsed.some((n) => typeof n !== 'number')
    ) {
      throw new Error('The vector must be a JSON array of numbers, like [0.1, 0.2].');
    }
    vector = parsed as number[];
  }
  const text = f.text.trim();
  if (text && (f.hybrid || !vector)) {
    retrievers.push({ text: { query: { queryString: { query: text } }, k: f.limit } });
  }
  if (vector) {
    retrievers.push({ vector: { field: f.vectorField, query: vector, k: f.limit } });
  }
  const ir: JsonObject = { limit: f.limit };
  if (retrievers.length > 0) ir.retrievers = retrievers;
  if (retrievers.length > 1) ir.fusion = { rrf: {} };
  if (f.filter.trim()) {
    try {
      ir.filter = JSON.parse(f.filter);
    } catch {
      throw new Error('The filter is not valid JSON.');
    }
  }
  return ir;
}

export function SearchTab({
  client,
  ns,
  coll,
  schema,
}: {
  client: DataClient;
  ns: string;
  coll: string;
  schema: JsonObject;
}) {
  const vectors = vectorFields(schema);
  const [form, setForm] = useState<SearchForm>({
    text: '',
    vector: '',
    vectorField: vectors[0]?.name ?? '',
    filter: '',
    limit: 10,
    hybrid: false,
  });
  const [hits, setHits] = useState<SearchHit[]>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const set = <K extends keyof SearchForm>(k: K, v: SearchForm[K]) => setForm({ ...form, [k]: v });

  const run = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    setBusy(true);
    try {
      setHits(await client.search(ns, coll, buildSearchIr(form)));
    } catch (err) {
      setHits(undefined);
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="ds-stack">
      <Card title="Query">
        <form className="ds-form" onSubmit={run}>
          <Field label="Text">
            {(p) => (
              <Input
                {...p}
                value={form.text}
                onChange={(e) => set('text', e.target.value)}
                placeholder="refund policy"
              />
            )}
          </Field>
          <div className="ds-field-row">
            <Field label="Vector" hint="A JSON array, for example [0.1, 0.2, 0.3].">
              {(p) => (
                <Input
                  {...p}
                  className="ds-mono"
                  value={form.vector}
                  onChange={(e) => set('vector', e.target.value)}
                />
              )}
            </Field>
            {vectors.length > 0 && (
              <Field label="Vector field">
                {(p) => (
                  <Select
                    {...p}
                    value={form.vectorField}
                    onChange={(e) => set('vectorField', e.target.value)}
                  >
                    {vectors.map((v) => (
                      <option key={v.name}>{v.name}</option>
                    ))}
                  </Select>
                )}
              </Field>
            )}
          </div>
          <Field
            label="Filter"
            hint='A filter in the query IR, for example {"term":{"field":"tenant","value":"a"}}.'
          >
            {(p) => (
              <Textarea
                {...p}
                className="ds-mono"
                rows={3}
                value={form.filter}
                onChange={(e) => set('filter', e.target.value)}
              />
            )}
          </Field>
          <div className="ds-field-row">
            <Field label="Limit">
              {(p) => (
                <Input
                  {...p}
                  type="number"
                  min={1}
                  max={1000}
                  value={form.limit}
                  onChange={(e) => set('limit', Math.max(1, Number(e.target.value) || 10))}
                />
              )}
            </Field>
            <Checkbox
              label="Hybrid (fuse text and vector)"
              checked={form.hybrid}
              onChange={(e) => set('hybrid', e.target.checked)}
            />
          </div>
          <div>
            <Button type="submit" variant="primary" disabled={busy}>
              Search
            </Button>
          </div>
        </form>
      </Card>
      {error && <ErrorNotice title="Search failed" message={error} />}
      {hits && (
        <Card title={`${hits.length} results`} flush>
          <Table<SearchHit>
            caption="Search results"
            rows={hits}
            rowKey={(h) => h.id}
            empty={<Empty title="No matches">Nothing matched this query.</Empty>}
            columns={[
              { key: 'id', header: 'id', cell: (h) => <code>{h.id}</code> },
              {
                key: 'score',
                header: 'Score',
                numeric: true,
                cell: (h) => <span data-testid="score">{h.score.toFixed(4)}</span>,
              },
              {
                key: 'source',
                header: 'Source',
                cell: (h) => <code className="ds-cell">{JSON.stringify(h.source)}</code>,
              },
            ]}
          />
        </Card>
      )}
    </div>
  );
}
