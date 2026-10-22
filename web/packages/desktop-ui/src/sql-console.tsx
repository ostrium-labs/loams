import type { IpcResult, SqlResult } from '@loams/desktop/contracts';
import type { SqlDialect } from '@loams/desktop/sql-lex';
import {
  Button,
  formatSql,
  Notice,
  RunHint,
  SqlEditor,
  type SqlEditorHandle,
  Table,
} from '@loams/ui';
import { useId, useRef, useState } from 'react';

export const MAX_ROWS = 1000;
/** Longest cell text rendered; longer values are clipped with an ellipsis. */
export const MAX_CELL_CHARS = 2048;

type Outcome =
  | { state: 'idle' }
  | { state: 'running' }
  | { state: 'done'; result: SqlResult }
  | { state: 'error'; code: string; message: string };

function cell(v: unknown) {
  if (v === null || v === undefined) return <span className="text-faint">NULL</span>;
  const text = typeof v === 'object' ? (JSON.stringify(v) ?? String(v)) : String(v);
  return text.length > MAX_CELL_CHARS ? `${text.slice(0, MAX_CELL_CHARS)}…` : text;
}

/**
 * An SQL editor with a result grid. Run with Ctrl/Cmd+Enter. A statement that is not a
 * plain read is confirmed by the main process (a native dialog), not here: one prompt.
 */
export function SqlConsole({
  dialect,
  run,
  initial = '',
  placeholder = 'SELECT 1',
}: {
  /** The SQL dialect; main uses the same scanner to decide what needs a confirm. */
  dialect: SqlDialect;
  run: (sql: string) => Promise<IpcResult<SqlResult>>;
  initial?: string;
  placeholder?: string;
}) {
  const [sql, setSql] = useState(initial);
  const [out, setOut] = useState<Outcome>({ state: 'idle' });
  const seq = useRef(0);
  const labelId = useId();
  const editor = useRef<SqlEditorHandle>(null);

  async function exec(text: string) {
    const mine = ++seq.current;
    setOut({ state: 'running' });
    try {
      const r = await run(text);
      if (mine !== seq.current) return;
      // The user said no in main's confirm: nothing ran, nothing to report.
      if (!r.ok && r.code === 'cancelled') {
        setOut({ state: 'idle' });
        return;
      }
      setOut(
        r.ok
          ? { state: 'done', result: r.value }
          : { state: 'error', code: r.code, message: r.message },
      );
    } catch (e) {
      if (mine === seq.current) {
        setOut({
          state: 'error',
          code: 'error',
          message: e instanceof Error ? e.message : String(e),
        });
      }
    }
  }

  function submit(statement = sql) {
    const text = statement.trim();
    if (!text || out.state === 'running') return;
    void exec(text);
  }

  // The renderer enforces the cap itself, whatever the backend sent.
  const raw = out.state === 'done' ? out.result : undefined;
  const result = raw && {
    ...raw,
    rows: raw.rows.length > MAX_ROWS ? raw.rows.slice(0, MAX_ROWS) : raw.rows,
    truncated: raw.truncated || raw.rows.length > MAX_ROWS,
  };
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-2">
        <span id={labelId} className="text-body font-medium">
          SQL editor
        </span>
        <div className="loams-sql-frame">
          <div className="loams-sql-toolbar">
            <Button
              variant="primary"
              size="sm"
              disabled={!sql.trim() || out.state === 'running'}
              onClick={() => submit()}
            >
              {out.state === 'running' ? 'Running…' : 'Run'}
            </Button>
            <Button
              size="sm"
              disabled={!sql.trim() || out.state === 'running'}
              onClick={() => submit(editor.current?.selectionOrStatement() || sql)}
            >
              Run selection
            </Button>
            <span className="loams-sql-toolbar-spacer" />
            <RunHint />
            <Button
              size="sm"
              variant="quiet"
              onClick={() => editor.current?.replace(formatSql(sql))}
            >
              Format
            </Button>
          </div>
          <SqlEditor
            handle={editor}
            label="SQL editor"
            dialect={dialect === 'mysql' ? 'mysql' : 'postgres'}
            placeholder={placeholder}
            value={sql}
            onChange={setSql}
            onRun={(text) => submit(text)}
          />
        </div>
      </div>

      {out.state === 'error' && (
        <Notice tone="danger" title={`Query failed (${out.code})`}>
          {out.message}
        </Notice>
      )}
      {result && (
        <div className="flex flex-col gap-2">
          <p className="m-0 text-body text-muted">
            {result.columns.length === 0
              ? `Statement ran in ${result.elapsedMs} ms.`
              : `${result.rows.length.toLocaleString('en-US')} row${result.rows.length === 1 ? '' : 's'} in ${result.elapsedMs} ms`}
          </p>
          {result.truncated && (
            <Notice
              tone="warn"
              title={`Result truncated at ${MAX_ROWS.toLocaleString('en-US')} rows`}
            >
              Only the first {MAX_ROWS.toLocaleString('en-US')} rows are shown. Add a LIMIT or a
              WHERE clause to narrow the query.
            </Notice>
          )}
          {result.columns.length > 0 && (
            <div className="overflow-auto">
              <Table
                caption="Query result"
                rows={result.rows.map((r, i) => ({ r, i }))}
                rowKey={(row) => String(row.i)}
                columns={result.columns.map((name, c) => ({
                  key: `${c}`,
                  header: <span className={/* mono-ok: sql */ 'font-mono'}>{name}</span>,
                  cell: (row: { r: unknown[] }) => (
                    <span className={/* mono-ok: sql */ 'font-mono text-small'}>
                      {cell(row.r[c])}
                    </span>
                  ),
                }))}
                empty={<p className="m-0 text-body text-muted">No rows.</p>}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
