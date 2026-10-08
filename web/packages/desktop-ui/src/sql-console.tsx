import type { IpcResult, SqlResult } from '@loams/desktop/contracts';
import { isWrite, type SqlDialect } from '@loams/desktop/sql-lex';
import { Button, Dialog, Notice, Table, Textarea } from '@loams/ui';
import { type KeyboardEvent, useId, useRef, useState } from 'react';

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

/** An SQL editor with a result grid. Writes ask first. Run with Ctrl/Cmd+Enter. */
export function SqlConsole({
  dialect,
  run,
  initial = '',
  placeholder = 'SELECT 1',
}: {
  dialect: SqlDialect;
  run: (sql: string) => Promise<IpcResult<SqlResult>>;
  initial?: string;
  placeholder?: string;
}) {
  const [sql, setSql] = useState(initial);
  const [out, setOut] = useState<Outcome>({ state: 'idle' });
  const [confirm, setConfirm] = useState<string>();
  const seq = useRef(0);
  const labelId = useId();

  async function exec(text: string) {
    const mine = ++seq.current;
    setOut({ state: 'running' });
    try {
      const r = await run(text);
      if (mine !== seq.current) return;
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

  function submit() {
    const text = sql.trim();
    if (!text || out.state === 'running') return;
    if (isWrite(text, dialect)) setConfirm(text);
    else void exec(text);
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      submit();
    }
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
        <label id={labelId} htmlFor={`${labelId}-editor`} className="text-sm font-medium">
          SQL editor
        </label>
        <Textarea
          id={`${labelId}-editor`}
          className="box-border min-h-32 w-full font-mono text-sm"
          spellCheck={false}
          rows={6}
          placeholder={placeholder}
          value={sql}
          onChange={(e) => setSql(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <div className="flex items-center gap-3">
          <Button
            variant="primary"
            disabled={!sql.trim() || out.state === 'running'}
            onClick={submit}
          >
            {out.state === 'running' ? 'Running…' : 'Run'}
          </Button>
          <span className="text-xs text-faint">Ctrl/Cmd+Enter</span>
        </div>
      </div>

      {out.state === 'error' && (
        <Notice tone="danger" title={`Query failed (${out.code})`}>
          {out.message}
        </Notice>
      )}
      {result && (
        <div className="flex flex-col gap-2">
          <p className="m-0 text-sm text-muted">
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
                  header: <span className="font-mono">{name}</span>,
                  cell: (row: { r: unknown[] }) => (
                    <span className="font-mono text-xs">{cell(row.r[c])}</span>
                  ),
                }))}
                empty={<p className="m-0 text-sm text-muted">No rows.</p>}
              />
            </div>
          )}
        </div>
      )}

      <Dialog
        open={confirm !== undefined}
        onClose={() => setConfirm(undefined)}
        title="Run a statement that changes data?"
        footer={
          <>
            <Button onClick={() => setConfirm(undefined)}>Cancel</Button>
            <Button
              variant="danger"
              onClick={() => {
                const text = confirm;
                setConfirm(undefined);
                if (text) void exec(text);
              }}
            >
              Run anyway
            </Button>
          </>
        }
      >
        <p className="m-0 text-sm text-muted">
          This does not look like a plain SELECT, SHOW or EXPLAIN. It runs against your local stack
          and may change data or schema. This check is a convenience, not a guarantee: a SELECT can
          still call a function with side effects.
        </p>
        <pre className="mt-3 max-h-48 overflow-auto font-mono text-xs">{confirm}</pre>
      </Dialog>
    </div>
  );
}
