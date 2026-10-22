// The editor (§48 §18.2): the statement in GQL (or Cypher when the graph enables it),
// parameters as a JSON object, the read-only toggle (on by default), Run, Explain,
// Profile and Cancel, and the history. A refused statement shows its GQLSTATUS and
// reason, and the span the server names (`line`/`column`/`length`, 1-based, counted in
// characters, R7.4) is underlined in the editor and shown under it. The editor is the
// console's shared query editor in its GQL mode: highlighting, and completion of keywords,
// the graph's labels, edge types and property keys.

import {
  Button,
  Checkbox,
  positionOfOffset,
  RunHint,
  Select,
  SqlEditor,
  type SqlPosition,
  type SqlSchema,
  Textarea,
} from '@loams/ui';
import { useId, useMemo } from 'react';
import type { GraphFailure, Language } from './client.js';

export interface EditorProps {
  statement: string;
  onStatement: (s: string) => void;
  parameters: string;
  onParameters: (s: string) => void;
  language: Language;
  onLanguage: (l: Language) => void;
  /** Whether the selected graph enables Cypher. */
  cypher: boolean;
  readOnly: boolean;
  onReadOnly: (on: boolean) => void;
  busy: boolean;
  onRun: () => void;
  onExplain: () => void;
  onProfile: () => void;
  onCancel: () => void;
  /** The last refusal, with the statement it refused. */
  failure?: { failure: GraphFailure; statement: string };
  history: string[];
  /** Labels, edge types and property keys of the selected graph, for completion. */
  schema?: SqlSchema;
}

/** The UTF-16 range of a 1-based line/column/length counted in code points, or undefined. */
export function spanRange(
  text: string,
  pos: { line: number; column: number; length: number },
): [number, number] | undefined {
  const lines = text.split('\n');
  const line = lines[pos.line - 1];
  if (line === undefined) return undefined;
  const offset = lines.slice(0, pos.line - 1).reduce((n, l) => n + l.length + 1, 0);
  const cps = Array.from(line);
  if (pos.column - 1 > cps.length) return undefined;
  const start = cps.slice(0, pos.column - 1).join('').length;
  const end = cps.slice(0, pos.column - 1 + pos.length).join('').length;
  return [offset + start, offset + Math.max(end, start)];
}

function ErrorSpan({
  statement,
  position,
}: {
  statement: string;
  position: NonNullable<GraphFailure['position']>;
}) {
  const line = statement.split('\n')[position.line - 1];
  if (line === undefined) return null;
  const cps = Array.from(line);
  const before = cps.slice(0, position.column - 1).join('');
  const span = cps.slice(position.column - 1, position.column - 1 + position.length).join('');
  const after = cps.slice(position.column - 1 + position.length).join('');
  return (
    <pre
      className={
        /* mono-ok: code */ 'm-0 overflow-auto rounded-md bg-surface p-2 font-mono text-[13px]'
      }
      data-testid="error-span"
    >
      <span className="select-none text-faint">{position.line} │ </span>
      {before}
      <mark className="bg-danger-soft text-ink underline decoration-danger decoration-wavy">
        {span || ' '}
      </mark>
      {after}
    </pre>
  );
}

export function FailureView({ failure, statement }: { failure: GraphFailure; statement?: string }) {
  const where = failure.position
    ? `line ${failure.position.line}, column ${failure.position.column}`
    : undefined;
  return (
    <div
      className="flex flex-col gap-2 rounded-md border border-solid border-danger bg-danger-soft p-3"
      role="alert"
    >
      <p className="m-0 font-medium">{failure.message || 'The server refused the call.'}</p>
      <p className="m-0 text-body text-muted">
        {[
          failure.gqlstatus && `GQLSTATUS ${failure.gqlstatus}`,
          failure.reason ?? failure.codeName,
          where,
        ]
          .filter(Boolean)
          .join(' · ')}
      </p>
      {failure.position && statement !== undefined && (
        <ErrorSpan statement={statement} position={failure.position} />
      )}
    </div>
  );
}

/** The editor position (UTF-16) of the span the server named (code points). */
function errorPosition(
  statement: string,
  failure?: { failure: GraphFailure; statement: string },
): SqlPosition | undefined {
  const pos = failure?.failure.position;
  if (!pos || failure.statement !== statement) return undefined;
  const range = spanRange(statement, pos);
  if (!range) return undefined;
  return { ...positionOfOffset(statement, range[0]), length: Math.max(1, range[1] - range[0]) };
}

export function Editor(p: EditorProps) {
  const ids = useId();
  const errorId = `${ids}-error`;
  const errorAt = useMemo(() => errorPosition(p.statement, p.failure), [p.statement, p.failure]);

  return (
    <section className="flex flex-col gap-3" aria-label="Editor">
      <div className="flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-2 text-body" htmlFor={`${ids}-lang`}>
          <span className="text-muted">Language</span>
          <Select
            id={`${ids}-lang`}
            value={p.language}
            onChange={(e) => p.onLanguage(e.target.value as Language)}
          >
            <option value="gql">GQL</option>
            {p.cypher && <option value="cypher">Cypher</option>}
          </Select>
        </label>
        <Checkbox
          label="Read-only"
          checked={p.readOnly}
          onChange={(e) => p.onReadOnly(e.target.checked)}
        />
        {p.history.length > 0 && (
          <label className="flex items-center gap-2 text-body" htmlFor={`${ids}-hist`}>
            <span className="text-muted">History</span>
            <Select
              id={`${ids}-hist`}
              value=""
              onChange={(e) => {
                if (e.target.value) p.onStatement(e.target.value);
              }}
            >
              <option value="">Recent statements…</option>
              {p.history.map((s) => (
                <option key={s} value={s}>
                  {s.length > 80 ? `${s.slice(0, 79)}…` : s}
                </option>
              ))}
            </Select>
          </label>
        )}
      </div>
      <div className="loams-sql-frame">
        <div className="loams-sql-toolbar" role="toolbar" aria-label="Statement actions">
          <Button
            variant="primary"
            size="sm"
            onClick={p.onRun}
            disabled={p.busy || !p.statement.trim()}
          >
            Run
          </Button>
          <Button size="sm" onClick={p.onExplain} disabled={p.busy || !p.statement.trim()}>
            Explain
          </Button>
          <Button size="sm" onClick={p.onProfile} disabled={p.busy || !p.statement.trim()}>
            Profile
          </Button>
          {p.busy && (
            <Button size="sm" variant="quiet" onClick={p.onCancel}>
              Cancel
            </Button>
          )}
          <span className="loams-sql-toolbar-spacer" />
          <RunHint selection={false} />
        </div>
        <SqlEditor
          label="Statement"
          dialect="gql"
          schema={p.schema}
          value={p.statement}
          onChange={p.onStatement}
          onRun={() => {
            if (!p.busy && p.statement.trim()) p.onRun();
          }}
          describedBy={p.failure ? errorId : undefined}
          invalid={!!p.failure?.failure.position}
          errorAt={errorAt}
          minLines={6}
          placeholder="MATCH (n) RETURN n LIMIT 10"
        />
      </div>
      <details>
        <summary className="cursor-pointer text-body text-muted">Parameters</summary>
        <Textarea
          aria-label="Parameters (JSON object)"
          className={/* mono-ok: code */ 'mt-2 box-border w-full resize-y font-mono text-[13px]'}
          rows={3}
          spellCheck={false}
          placeholder='{"name": "Keanu Reeves"}'
          value={p.parameters}
          onChange={(e) => p.onParameters(e.target.value)}
        />
        <p className="m-0 text-small text-muted">
          Parameters are sent with the statement and never kept in the history.
        </p>
      </details>
      {p.failure && (
        <div id={errorId}>
          <FailureView failure={p.failure.failure} statement={p.failure.statement} />
        </div>
      )}
    </section>
  );
}
