import type { LoamsDesktopApi, WesqlTable } from '@loams/desktop/contracts';
import { Button, Empty, Notice, Table } from '@loams/ui';
import { useEffect, useState } from 'react';

type Wesql = LoamsDesktopApi['wesql'];
type Load<T> =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; data: T };

function useLoad<T>(
  fn: () => Promise<{ ok: true; value: T } | { ok: false; message: string }>,
  key: string,
) {
  const [v, setV] = useState<Load<T>>({ state: 'loading' });
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` stands for `fn`'s inputs.
  useEffect(() => {
    let live = true;
    setV({ state: 'loading' });
    fn()
      .then(
        (r) =>
          live &&
          setV(r.ok ? { state: 'ready', data: r.value } : { state: 'error', message: r.message }),
      )
      .catch(
        (e) =>
          live && setV({ state: 'error', message: e instanceof Error ? e.message : String(e) }),
      );
    return () => {
      live = false;
    };
  }, [key]);
  return v;
}

function Tables({ wesql, schema }: { wesql: Wesql; schema: string }) {
  const tables = useLoad(() => wesql.tables(schema), schema);
  if (tables.state === 'loading') return <p className="text-sm text-muted">Loading tables…</p>;
  if (tables.state === 'error') {
    return (
      <Notice tone="danger" title={`Could not load tables in ${schema}`}>
        {tables.message}
      </Notice>
    );
  }
  return (
    <Table<WesqlTable>
      caption={`Tables in ${schema}`}
      rows={tables.data}
      rowKey={(t) => t.name}
      empty={<Empty title="No tables">This schema has no tables.</Empty>}
      columns={[
        { key: 'name', header: 'Table', cell: (t) => <span className="font-mono">{t.name}</span> },
        { key: 'engine', header: 'Engine', cell: (t) => t.engine },
        { key: 'rows', header: 'Rows', numeric: true, cell: (t) => t.rows.toLocaleString('en-US') },
      ]}
    />
  );
}

/** The Schemas tab: the schema list, then the tables of the chosen schema. */
export function Schemas({ wesql }: { wesql: Wesql }) {
  const schemas = useLoad(() => wesql.schemas(), 'schemas');
  const [picked, setPicked] = useState<string>();
  if (schemas.state === 'loading') return <p className="text-sm text-muted">Loading schemas…</p>;
  if (schemas.state === 'error') {
    return (
      <Notice tone="danger" title="Could not load schemas">
        {schemas.message}
      </Notice>
    );
  }
  if (schemas.data.length === 0) {
    return <Empty title="No schemas">The server reports no schemas.</Empty>;
  }
  const current = picked ?? schemas.data[0]?.name;
  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-[14rem_1fr]">
      <nav aria-label="Schemas" className="flex flex-col gap-1">
        {schemas.data.map((s) => (
          <Button
            key={s.name}
            variant={s.name === current ? 'primary' : 'quiet'}
            aria-pressed={s.name === current}
            className="justify-start font-mono"
            onClick={() => setPicked(s.name)}
          >
            {s.name}
          </Button>
        ))}
      </nav>
      <div className="min-w-0">{current && <Tables wesql={wesql} schema={current} />}</div>
    </div>
  );
}
