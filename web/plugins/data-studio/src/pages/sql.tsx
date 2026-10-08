import type { JsonValue } from '@bufbuild/protobuf';
import { Badge, Button, Card, Empty, Notice, Textarea } from '@loams/ui';
import { type KeyboardEvent, useState } from 'react';
import type { DataClient, SqlResult } from '../client.js';
import { PageHead, Tabs } from '../shared.js';

function show(v: JsonValue): string {
  if (v === null) return 'NULL';
  return typeof v === 'string' ? v : JSON.stringify(v);
}

/** `/data/:ns/sql`: a read-only SQL editor over the namespace. */
export function SqlPage({
  client,
  ns,
  navigate,
}: {
  client: DataClient;
  ns: string;
  navigate: (to: string) => void;
}) {
  const [text, setText] = useState('SELECT 1');
  const [result, setResult] = useState<SqlResult>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const run = async () => {
    setBusy(true);
    setError(undefined);
    try {
      setResult(await client.sql(ns, text));
    } catch (e) {
      setResult(undefined);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void run();
    }
  };

  return (
    <div className="lc-page ds-page">
      <PageHead
        crumbs={
          <>
            <a href="#/data">Data</a> / <a href={`#/data/${encodeURIComponent(ns)}`}>{ns}</a>
          </>
        }
        title="SQL"
        subtitle={`One read-only statement against ${ns}.`}
        actions={
          <Button variant="primary" onClick={run} disabled={busy || !text.trim()}>
            Run
          </Button>
        }
      />
      <Tabs ns={ns} active="sql" navigate={navigate} />
      <Card>
        <Textarea
          aria-label="SQL"
          className="ds-editor"
          rows={8}
          spellCheck={false}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKey}
        />
        <p className="lc-muted">Press Ctrl or Cmd + Enter to run.</p>
      </Card>
      {error && (
        <Notice tone="danger" title="Query failed">
          {error}
        </Notice>
      )}
      {result && (
        <Card title={`${result.rows.length} rows`} flush>
          {result.truncated && (
            <Notice tone="warn" title="Truncated">
              The server's row limit cut this result.
            </Notice>
          )}
          {result.rows.length === 0 ? (
            <Empty title="No rows">The query returned nothing.</Empty>
          ) : (
            <div className="loams-table-wrap">
              <table className="loams-table">
                <thead>
                  <tr>
                    {result.columns.map((c) => (
                      <th key={c.name}>
                        {c.name} <Badge>{c.type}</Badge>
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {result.rows.map((row, i) => (
                    // biome-ignore lint/suspicious/noArrayIndexKey: result rows have no key
                    <tr key={i}>
                      {row.map((v, j) => (
                        // biome-ignore lint/suspicious/noArrayIndexKey: positional cells
                        <td key={j}>
                          <code>{show(v)}</code>
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
