import type { JsonObject, JsonValue } from '@bufbuild/protobuf';
import type { documents } from '@loams/proto';
import { Badge, Button, Card, Empty, Stat, Stats, Table } from '@loams/ui';
import { useCallback, useEffect, useState } from 'react';
import { type DataClient, type DocRow, schemaOf } from '../client.js';
import { ErrorNotice, errorText, PageHead, type Tab, Tabs, useLoad } from '../shared.js';
import { IngestTab } from './ingest.js';
import { SearchTab } from './search.js';

const PAGE_SIZE = 25;

function cell(v: JsonValue | undefined): string {
  if (v === undefined || v === null) return '';
  const text = typeof v === 'string' ? v : JSON.stringify(v);
  return text.length > 80 ? `${text.slice(0, 79)}…` : text;
}

export function DocumentsTab({
  client,
  ns,
  coll,
}: {
  client: DataClient;
  ns: string;
  coll: string;
}) {
  // The cursor of every page shown so far; the last one is the page on screen.
  const [cursors, setCursors] = useState<(documents.DocumentId | undefined)[]>([undefined]);
  const [selected, setSelected] = useState<DocRow>();
  const cursor = cursors[cursors.length - 1];
  const [page] = useLoad(
    () => client.scroll(ns, coll, { pageToken: cursor, limit: PAGE_SIZE }),
    [client, ns, coll, cursors.length],
  );
  const goTo = (next: typeof cursors) => {
    setSelected(undefined);
    setCursors(next);
  };

  if (page.state === 'loading') return <p className="lc-muted ds-pad">Loading documents…</p>;
  if (page.state === 'error')
    return <ErrorNotice title="Could not read documents" message={page.message} />;
  const { rows, next } = page.data;
  const keys: string[] = [];
  for (const r of rows) for (const k of Object.keys(r.source)) if (!keys.includes(k)) keys.push(k);
  const shown = keys.slice(0, 5);

  return (
    <div className={selected ? 'ds-split ds-split-open' : 'ds-split'}>
      <Card flush>
        <Table<DocRow>
          caption="Documents"
          rows={rows}
          rowKey={(r) => r.id}
          onRowClick={setSelected}
          empty={<Empty title="No documents">Use the Ingest tab to load some.</Empty>}
          columns={[
            {
              key: 'id',
              header: 'id',
              cell: (r) => (
                <button type="button" className="ds-cell-btn" onClick={() => setSelected(r)}>
                  <code>{r.id}</code>
                </button>
              ),
            },
            ...shown.map((k) => ({
              key: k,
              header: k,
              cell: (r: DocRow) => <span className="ds-cell">{cell(r.source[k])}</span>,
            })),
          ]}
        />
        <div className="ds-pager">
          <span className="lc-muted">
            Page {cursors.length}
            {rows.length > 0 && ` · ${rows.length} documents`}
          </span>
          <Button
            size="sm"
            disabled={cursors.length === 1}
            onClick={() => goTo(cursors.slice(0, -1))}
          >
            Previous
          </Button>
          <Button size="sm" disabled={!next} onClick={() => next && goTo([...cursors, next])}>
            Next
          </Button>
        </div>
      </Card>
      {selected && (
        <aside className="ds-panel" aria-label="Document">
          <div className="ds-panel-head">
            <strong>
              <code>{selected.id}</code>
            </strong>
            <Button variant="quiet" size="sm" onClick={() => setSelected(undefined)}>
              Close
            </Button>
          </div>
          <pre className="ds-json">
            {JSON.stringify(
              { id: selected.id, source: selected.source, vectors: selected.vectors },
              null,
              2,
            )}
          </pre>
        </aside>
      )}
    </div>
  );
}

export function SchemaTab({ schema }: { schema: JsonObject }) {
  const fields = Array.isArray(schema.fields) ? (schema.fields as JsonObject[]) : [];
  const vectors = Array.isArray(schema.vectors) ? (schema.vectors as JsonObject[]) : [];
  const kindName = (k: JsonValue | undefined) =>
    typeof k === 'string' ? k : k && typeof k === 'object' ? Object.keys(k)[0] : '';
  return (
    <div className="ds-stack">
      <Card title="Fields" flush>
        <Table<JsonObject>
          caption="Fields"
          rows={fields}
          rowKey={(f) => String(f.name)}
          empty={<Empty title="No typed fields">This collection indexes its source only.</Empty>}
          columns={[
            { key: 'name', header: 'Name', cell: (f) => <code>{String(f.name)}</code> },
            { key: 'kind', header: 'Type', cell: (f) => <Badge>{kindName(f.kind)}</Badge> },
            { key: 'idx', header: 'Indexed', cell: (f) => (f.indexed ? 'yes' : 'no') },
            { key: 'fast', header: 'Fast', cell: (f) => (f.fast ? 'yes' : 'no') },
          ]}
        />
      </Card>
      {vectors.length > 0 && (
        <Card title="Vectors" flush>
          <Table<JsonObject>
            caption="Vectors"
            rows={vectors}
            rowKey={(v) => String(v.name)}
            columns={[
              { key: 'name', header: 'Name', cell: (v) => <code>{String(v.name)}</code> },
              { key: 'dim', header: 'Dimension', numeric: true, cell: (v) => String(v.dim) },
              { key: 'dist', header: 'Metric', cell: (v) => String(v.distance) },
            ]}
          />
        </Card>
      )}
      <Card title="Schema JSON">
        <pre className="ds-json">{JSON.stringify(schema, null, 2)}</pre>
      </Card>
    </div>
  );
}

/** `/data/:ns/:coll[/search|/ingest|/schema]`: one collection with its tabs. */
export function CollectionPage({
  client,
  ns,
  coll,
  tab,
  navigate,
}: {
  client: DataClient;
  ns: string;
  coll: string;
  tab: Exclude<Tab, 'sql'>;
  navigate: (to: string) => void;
}) {
  const [info] = useLoad(() => client.getCollection(ns, coll), [client, ns, coll]);
  const [count, setCount] = useState<number>();
  const refreshCount = useCallback(
    () => client.count(ns, coll).then(setCount, (e) => console.warn(errorText(e))),
    [client, ns, coll],
  );
  useEffect(() => {
    setCount(undefined);
    void refreshCount();
  }, [refreshCount]);

  return (
    <div className="lc-page ds-page">
      <PageHead
        crumbs={
          <>
            <a href="#/data">Data</a> / <a href={`#/data/${encodeURIComponent(ns)}`}>{ns}</a>
          </>
        }
        title={<code>{coll}</code>}
        subtitle={info.state === 'ready' ? `Namespace ${ns}` : undefined}
      />
      {info.state === 'error' && (
        <ErrorNotice title="Could not read the collection" message={info.message} />
      )}
      {info.state === 'ready' && (
        <Stats>
          <Stat label="Documents" value={(count ?? info.data.liveDocCount).toString()} />
          <Stat label="Version" value={info.data.manifestVersion.toString()} />
          <Stat label="Partitions" value={info.data.partitions.toString()} />
        </Stats>
      )}
      <Tabs ns={ns} coll={coll} active={tab} navigate={navigate} />
      {tab === 'documents' && <DocumentsTab client={client} ns={ns} coll={coll} />}
      {tab === 'search' && info.state === 'ready' && (
        <SearchTab client={client} ns={ns} coll={coll} schema={schemaOf(info.data)} />
      )}
      {tab === 'ingest' && info.state === 'ready' && (
        <IngestTab
          client={client}
          ns={ns}
          coll={coll}
          schema={schemaOf(info.data)}
          onDone={refreshCount}
        />
      )}
      {tab === 'schema' && info.state === 'ready' && <SchemaTab schema={schemaOf(info.data)} />}
    </div>
  );
}
