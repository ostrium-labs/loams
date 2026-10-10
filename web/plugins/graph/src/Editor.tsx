// The editor (§48 §18.2): the statement in GQL (or Cypher when the graph enables it),
// parameters as a JSON object, the read-only toggle (on by default), Run, Explain,
// Profile and Cancel, and the history. A refused statement shows its GQLSTATUS and
// reason, and the span the server names (`line`/`column`/`length`, 1-based, counted in
// characters, R7.4) is underlined under the editor and selected in it.

import { Button, Checkbox, Select, Textarea } from '@loams/ui';
import { type KeyboardEvent, useEffect, useId, useRef } from 'react';
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
      className="m-0 overflow-auto rounded-md bg-surface p-2 font-mono text-[13px]"
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
      <p className="m-0 text-sm text-muted">
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

export function Editor(p: EditorProps) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const ids = useId();
  const errorId = `${ids}-error`;

  // Select the span the server named, so the caret lands on it.
  useEffect(() => {
    const pos = p.failure?.failure.position;
    const el = ref.current;
    if (!pos || !el || el.value !== p.failure?.statement) return;
    const range = spanRange(el.value, pos);
    if (range) el.setSelectionRange(range[0], range[1]);
  }, [p.failure]);

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      if (!p.busy && p.statement.trim()) p.onRun();
    }
  };

  return (
    <section className="flex flex-col gap-3" aria-label="Editor">
      <div className="flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-2 text-sm" htmlFor={`${ids}-lang`}>
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
          <label className="flex items-center gap-2 text-sm" htmlFor={`${ids}-hist`}>
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
      <textarea
        ref={ref}
        aria-label="Statement"
        aria-describedby={p.failure ? errorId : undefined}
        aria-invalid={p.failure?.failure.position ? true : undefined}
        className="loams-input box-border w-full resize-y font-mono text-[13px]"
        rows={7}
        spellCheck={false}
        value={p.statement}
        onChange={(e) => p.onStatement(e.target.value)}
        onKeyDown={onKey}
      />
      <details>
        <summary className="cursor-pointer text-sm text-muted">Parameters</summary>
        <Textarea
          aria-label="Parameters (JSON object)"
          className="mt-2 box-border w-full resize-y font-mono text-[13px]"
          rows={3}
          spellCheck={false}
          placeholder='{"name": "Keanu Reeves"}'
          value={p.parameters}
          onChange={(e) => p.onParameters(e.target.value)}
        />
        <p className="m-0 text-xs text-muted">
          Parameters are sent with the statement and never kept in the history.
        </p>
      </details>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={p.onRun} disabled={p.busy || !p.statement.trim()}>
          Run
        </Button>
        <Button onClick={p.onExplain} disabled={p.busy || !p.statement.trim()}>
          Explain
        </Button>
        <Button onClick={p.onProfile} disabled={p.busy || !p.statement.trim()}>
          Profile
        </Button>
        {p.busy && (
          <Button variant="quiet" onClick={p.onCancel}>
            Cancel
          </Button>
        )}
        <span className="text-xs text-muted">Ctrl or Cmd + Enter runs.</span>
      </div>
      {p.failure && (
        <div id={errorId}>
          <FailureView failure={p.failure.failure} statement={p.failure.statement} />
        </div>
      )}
    </section>
  );
}
