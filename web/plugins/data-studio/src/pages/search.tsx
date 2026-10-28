// The collection's Search tab, in the manner of Kibana Discover: one query bar with
// suggestions, filter pills, a field sidebar, retrieval controls (Hybrid / Text / Vector,
// fusion, top-k) and results as cards or a table. The state lives in the URL.

import type { JsonObject } from '@bufbuild/protobuf';
import {
  Badge,
  Button,
  cx,
  Empty,
  HistoryMenu,
  Menu,
  MenuItem,
  MenuSeparator,
  Popover,
  Segmented,
  useQueryHistory,
} from '@loams/ui';
import { CircleAlert, LayoutList, Plus, Search, Table2, X } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { type DataClient, type SearchHit, toDocumentId } from '../client.js';
import { FieldSidebar } from '../search/field-sidebar.js';
import {
  clause,
  compileQuery,
  type FieldInfo,
  type KqlError,
  schemaFields,
} from '../search/kql.js';
import { QueryInput } from '../search/query-input.js';
import {
  buildRequest,
  type FusionMethod,
  fromParams,
  K_CHOICES,
  type Mode,
  type Pill,
  parseVector,
  type SearchState,
  toParams,
} from '../search/request.js';
import { type Ranks, Results } from '../search/results.js';
import type { SuggestContext } from '../search/suggest.js';
import { useFieldValues } from '../search/values.js';
import { errorText } from '../shared.js';

/** The query vector: pasted, or a document's ("find similar"). */
type QueryVector =
  | { from: 'paste'; values: number[] }
  | { from: 'doc'; id: string; values: number[] };

type Outcome =
  | { state: 'idle' }
  | { state: 'running' }
  | {
      state: 'done';
      hits: SearchHit[];
      total?: number;
      totalAtLeast: boolean;
      ms: number;
      plan: string;
      notes: string[];
      terms: string[];
      ranks?: Ranks;
      filter?: JsonObject;
    }
  | { state: 'error'; message: string };

type QueryProblem = { error: KqlError; source: string; text: string };

let pillSeq = 0;
const newPillId = () => `pill${++pillSeq}`;

export function SearchTab({
  client,
  ns,
  coll,
  schema,
  params = {},
  onParams,
}: {
  client: DataClient;
  ns: string;
  coll: string;
  schema: JsonObject;
  /** The route's query parameters: the shared state. */
  params?: Record<string, string | undefined>;
  /** Called with the query string of the state after each search, to keep the URL. */
  onParams?: (query: string) => void;
}) {
  const fields = useMemo(() => schemaFields(schema), [schema]);
  const vectorFields = fields.filter((f) => f.type === 'vector');
  const [state, setState] = useState<SearchState>(() => fromParams(params, fields));
  const [draft, setDraft] = useState(state.q);
  const [vector, setVector] = useState<QueryVector>();
  const [outcome, setOutcome] = useState<Outcome>({ state: 'idle' });
  const [problem, setProblem] = useState<QueryProblem>();
  const [editing, setEditing] = useState<string>();
  const values = useFieldValues(client, ns, coll, fields);
  const history = useQueryHistory(`search:${ns}`);
  const abort = useRef<AbortController | undefined>(undefined);
  const errorId = useId();
  const inputRef = useRef<HTMLInputElement>(null);

  const recent = useMemo(
    () =>
      history.items
        .map((h) => h.text)
        .filter((t) => t.trim() && compileQuery(t, fields).ok)
        .slice(0, 8),
    [history.items, fields],
  );

  const onContext = useCallback(
    (c: SuggestContext) => {
      if (c.kind === 'value') values.request(c.field, c.prefix);
    },
    [values],
  );

  const run = useCallback(
    async (next: SearchState, vec: QueryVector | undefined) => {
      setState(next);
      setDraft(next.q);
      const built = buildRequest(next, fields, vec?.values);
      if (!built.ok) {
        const text =
          built.source === 'query'
            ? next.q
            : (next.pills.find((p) => p.id === built.source)?.q ?? '');
        setProblem({ error: built.error, source: built.source, text });
        return;
      }
      setProblem(undefined);
      onParams?.(toParams(next, fields));
      abort.current?.abort();
      const ctrl = new AbortController();
      abort.current = ctrl;
      setOutcome({ state: 'running' });
      const { request } = built;
      const started = performance.now();
      try {
        const [main, text, vecList] = await Promise.all([
          client.search(ns, coll, request.ir, ctrl.signal),
          request.signals ? client.search(ns, coll, request.signals.text, ctrl.signal) : undefined,
          request.signals
            ? client.search(ns, coll, request.signals.vector, ctrl.signal)
            : undefined,
        ]);
        if (ctrl.signal.aborted) return;
        const ms = Math.round(performance.now() - started);
        const rank = (list: SearchHit[]) => new Map(list.map((h, i) => [h.id, i + 1]));
        setOutcome({
          state: 'done',
          hits: main.hits,
          total: main.total,
          totalAtLeast: main.totalAtLeast,
          ms,
          plan: request.plan,
          notes: request.notes,
          terms: request.terms,
          ranks:
            text && vecList ? { text: rank(text.hits), vector: rank(vecList.hits) } : undefined,
          filter: request.ir.filter as JsonObject | undefined,
        });
        if (next.q.trim())
          history.push({
            text: next.q.trim(),
            at: Date.now(),
            ok: true,
            elapsedMs: ms,
            rows: main.hits.length,
          });
      } catch (e) {
        if (ctrl.signal.aborted) return;
        setOutcome({ state: 'error', message: errorText(e) });
        if (next.q.trim()) history.push({ text: next.q.trim(), at: Date.now(), ok: false });
      }
    },
    [client, ns, coll, fields, onParams, history],
  );

  // The first search: the URL's state, reading the "similar to" document's vector first.
  // oxlint-disable-next-line react/exhaustive-deps -- once per collection
  useEffect(() => {
    const initial = fromParams(params, fields);
    if (initial.similarTo && initial.vectorField) {
      const id = initial.similarTo;
      client
        .vectorOf(ns, coll, toDocumentId(/^\d+$/.test(id) ? Number(id) : id), initial.vectorField)
        .then(
          (values) => {
            const v: QueryVector = { from: 'doc', id, values };
            setVector(v);
            void run(initial, v);
          },
          (e) => {
            setOutcome({
              state: 'error',
              message: `Could not read document ${id}: ${errorText(e)}`,
            });
            setState({ ...initial, similarTo: undefined });
          },
        );
    } else void run(initial, undefined);
    return () => abort.current?.abort();
  }, [ns, coll]);

  const submit = (q = draft) => void run({ ...state, q }, vector);
  const update = (patch: Partial<SearchState>, vec = vector) =>
    void run({ ...state, q: draft, ...patch }, vec);

  const addPill = (q: string, negate = false) =>
    update({ pills: [...state.pills, { id: newPillId(), q, negate, disabled: false }] });
  const filterValue = (field: string, value: string, negate: boolean) =>
    addPill(clause(field, value), negate);
  const setPill = (id: string, patch: Partial<Pill>) =>
    update({ pills: state.pills.map((p) => (p.id === id ? { ...p, ...patch } : p)) });
  const removePill = (id: string) => update({ pills: state.pills.filter((p) => p.id !== id) });

  const findSimilar = async (hit: SearchHit) => {
    if (!state.vectorField || !hit.pk) return;
    try {
      const values = await client.vectorOf(ns, coll, hit.pk, state.vectorField);
      const v: QueryVector = { from: 'doc', id: hit.id, values };
      setVector(v);
      update(
        {
          similarTo: hit.id,
          mode: state.mode === 'text' ? (draft.trim() ? 'hybrid' : 'vector') : state.mode,
        },
        v,
      );
    } catch (e) {
      setOutcome({ state: 'error', message: errorText(e) });
    }
  };

  const queryProblem = problem?.source === 'query' ? problem : undefined;
  const dim = vectorFields.find((f) => f.name === state.vectorField)?.dim;

  return (
    <div className="flex flex-col gap-3">
      {/* The query bar */}
      <div className="flex flex-wrap items-center gap-2">
        <QueryInput
          className="min-w-72 flex-1"
          inputRef={inputRef}
          label="Search query"
          placeholder={placeholderFor(fields)}
          value={draft}
          onChange={setDraft}
          onSubmit={submit}
          fields={fields}
          values={values.get}
          onContext={onContext}
          recent={recent}
          invalid={!!queryProblem}
          describedBy={queryProblem ? errorId : undefined}
          end={
            <Button
              size="sm"
              variant="primary"
              onClick={() => submit()}
              disabled={outcome.state === 'running'}
              aria-label="Search"
            >
              <Search size={14} aria-hidden /> Search
            </Button>
          }
        />
        <HistoryMenu items={history.items} onPick={(t) => submit(t)} onClear={history.clear} />
        <QueryHelp />
      </div>
      {queryProblem && (
        <ProblemView
          id={errorId}
          problem={queryProblem}
          onFocus={() => focusAt(inputRef.current, queryProblem.error)}
        />
      )}

      {/* Filter pills */}
      {/* oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- a labelled row of pills, not a form fieldset */}
      <div role="group" className="flex flex-wrap items-center gap-1.5" aria-label="Filters">
        {state.pills.map((p) =>
          editing === p.id ? (
            <PillEditor
              key={p.id}
              initial={p.q}
              fields={fields}
              values={values.get}
              onContext={onContext}
              onSave={(q) => {
                setEditing(undefined);
                setPill(p.id, { q });
              }}
              onCancel={() => setEditing(undefined)}
            />
          ) : (
            <FilterPill
              key={p.id}
              pill={p}
              invalid={problem?.source === p.id}
              onEdit={() => setEditing(p.id)}
              onNegate={() => setPill(p.id, { negate: !p.negate })}
              onDisable={() => setPill(p.id, { disabled: !p.disabled })}
              onRemove={() => removePill(p.id)}
            />
          ),
        )}
        {editing === 'new' ? (
          <PillEditor
            initial=""
            fields={fields}
            values={values.get}
            onContext={onContext}
            onSave={(q) => {
              setEditing(undefined);
              if (q.trim()) addPill(q.trim());
            }}
            onCancel={() => setEditing(undefined)}
          />
        ) : (
          <Button size="xs" variant="quiet" onClick={() => setEditing('new')}>
            <Plus size={12} aria-hidden /> Add filter
          </Button>
        )}
        {state.pills.length > 1 && (
          <Button size="xs" variant="quiet" onClick={() => update({ pills: [] })}>
            Clear filters
          </Button>
        )}
      </div>
      {problem && problem.source !== 'query' && (
        <ProblemView problem={problem} title="This filter has a problem" />
      )}

      {/* Retrieval controls */}
      <RetrievalBar
        state={state}
        vectorFields={vectorFields}
        vector={vector}
        dim={dim}
        onMode={(mode) => update({ mode })}
        onK={(k) => update({ k })}
        onFusion={(fusion) => update({ fusion })}
        onRrfK={(rrfK) => update({ rrfK })}
        onWeight={(textWeight) => update({ textWeight })}
        onVectorField={(vectorField) => {
          setVector(undefined);
          update({ vectorField, similarTo: undefined }, undefined);
        }}
        onVector={(v) => {
          setVector(v);
          update(
            {
              similarTo: v?.from === 'doc' ? v.id : undefined,
              mode: v && state.mode === 'text' ? 'hybrid' : state.mode,
            },
            v,
          );
        }}
      />

      <div className="grid grid-cols-1 gap-4 md:grid-cols-[220px_minmax(0,1fr)]">
        <FieldSidebar
          client={client}
          ns={ns}
          coll={coll}
          fields={fields}
          filter={outcome.state === 'done' ? outcome.filter : undefined}
          onFilter={filterValue}
          onExists={(field) => addPill(`${field}:*`)}
          columns={state.columns}
          onToggleColumn={(f) =>
            update({
              columns: state.columns.includes(f)
                ? state.columns.filter((c) => c !== f)
                : [...state.columns, f],
              view: 'table',
            })
          }
        />
        <section
          aria-label="Results"
          aria-busy={outcome.state === 'running'}
          className="flex min-w-0 flex-col gap-2"
        >
          <StatusLine outcome={outcome} view={state.view} onView={(view) => update({ view })} />
          {outcome.state === 'error' && (
            <div
              role="alert"
              className="flex items-start gap-2 rounded-md border border-danger bg-danger-soft px-3 py-2 text-body text-ink"
            >
              <CircleAlert size={16} aria-hidden className="mt-0.5 shrink-0 text-danger" />
              <div>
                <strong className="block">The engine refused this search</strong>
                {outcome.message}
              </div>
            </div>
          )}
          {outcome.state === 'done' && outcome.hits.length === 0 && (
            <Empty title="No matches">Nothing matched. Remove a filter or try other words.</Empty>
          )}
          {outcome.state === 'done' && outcome.hits.length > 0 && (
            <Results
              hits={outcome.hits}
              fields={fields}
              terms={outcome.terms}
              ranks={outcome.ranks}
              view={state.view}
              columns={state.columns}
              canSimilar={vectorFields.length > 0 && !!state.vectorField}
              onSimilar={(h) => void findSimilar(h)}
              onFilter={filterValue}
            />
          )}
        </section>
      </div>
    </div>
  );
}

function placeholderFor(fields: FieldInfo[]): string {
  const kw = fields.find((f) => f.type === 'keyword');
  const text = fields.some((f) => f.type === 'text');
  if (text && kw) return `Search: words, "a phrase", ${kw.name}:value, AND / OR / NOT`;
  if (kw) return `Filter: ${kw.name}:value, AND / OR / NOT`;
  return 'Search: words, "a phrase", field:value';
}

function focusAt(el: HTMLInputElement | null, e: KqlError) {
  if (!el) return;
  el.focus();
  el.setSelectionRange(e.from, Math.max(e.to, e.from));
}

function ProblemView({
  problem,
  id,
  title = 'The query has a problem',
  onFocus,
}: {
  problem: QueryProblem;
  id?: string;
  title?: string;
  onFocus?: () => void;
}) {
  const { error, text } = problem;
  const from = Math.min(error.from, text.length);
  const to = Math.max(Math.min(error.to, text.length), from);
  return (
    <div
      id={id}
      role="alert"
      className="flex flex-col gap-1 rounded-md border border-danger bg-danger-soft px-3 py-2 text-body text-ink"
    >
      <div className="flex items-start gap-2">
        <CircleAlert size={16} aria-hidden className="mt-0.5 shrink-0 text-danger" />
        <span className="flex-1">
          <strong>{title}:</strong> {error.message} Nothing was sent.
        </span>
        {onFocus && (
          <Button size="xs" variant="quiet" onClick={onFocus}>
            Show in the query
          </Button>
        )}
      </div>
      {text && (
        <code className="ml-6 overflow-x-auto whitespace-pre text-small">
          {text.slice(0, from)}
          <mark className="rounded-xs bg-danger px-0.5 text-bg underline decoration-wavy">
            {text.slice(from, to) || ' '}
          </mark>
          {text.slice(to)}
        </code>
      )}
    </div>
  );
}

function FilterPill({
  pill,
  invalid,
  onEdit,
  onNegate,
  onDisable,
  onRemove,
}: {
  pill: Pill;
  invalid: boolean;
  onEdit: () => void;
  onNegate: () => void;
  onDisable: () => void;
  onRemove: () => void;
}) {
  return (
    <span
      className={cx(
        'inline-flex max-w-full items-center rounded-pill border text-small',
        invalid
          ? 'border-danger bg-danger-soft'
          : pill.negate
            ? 'border-danger bg-danger-soft'
            : 'border-rule bg-raised',
        pill.disabled && 'opacity-60',
      )}
    >
      <Menu
        label={`Filter ${pill.q}`}
        align="start"
        trigger={
          <button
            type="button"
            className={cx(
              'inline-flex min-w-0 cursor-pointer items-center gap-1 border-0 bg-transparent py-1 pr-1 pl-2.5 text-ink',
              pill.disabled && 'line-through',
            )}
          >
            {pill.negate && <span className="font-semibold text-danger">NOT</span>}
            <code className="truncate">{pill.q}</code>
          </button>
        }
      >
        <MenuItem onSelect={onEdit}>Edit filter</MenuItem>
        <MenuItem onSelect={onNegate}>
          {pill.negate ? 'Include results' : 'Exclude results'}
        </MenuItem>
        <MenuItem onSelect={onDisable}>{pill.disabled ? 'Enable' : 'Temporarily disable'}</MenuItem>
        <MenuSeparator />
        <MenuItem onSelect={onRemove} tone="danger">
          Remove
        </MenuItem>
      </Menu>
      <button
        type="button"
        aria-label={`Remove filter ${pill.q}`}
        onClick={onRemove}
        className="mr-1 inline-flex size-5 cursor-pointer items-center justify-center rounded-pill border-0 bg-transparent text-muted hover:text-ink"
      >
        <X size={12} aria-hidden />
      </button>
    </span>
  );
}

function PillEditor({
  initial,
  fields,
  values,
  onContext,
  onSave,
  onCancel,
}: {
  initial: string;
  fields: FieldInfo[];
  values: (field: string, prefix: string) => { value: string; count?: number }[] | undefined;
  onContext: (c: SuggestContext) => void;
  onSave: (q: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial);
  const check = text.trim() ? compileQuery(text, fields, { scored: false }) : undefined;
  const err = check && !check.ok ? check.error : undefined;
  return (
    // Escape that the suggestion list did not take cancels the edit.
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- keyboard handling for the editor inside
    <span
      className="inline-flex w-96 max-w-full flex-col gap-1"
      onKeyDown={(e) => {
        if (e.key === 'Escape') onCancel();
      }}
    >
      <QueryInput
        size="sm"
        autoFocus
        label="Filter"
        placeholder="field:value"
        value={text}
        onChange={setText}
        onSubmit={(q) => {
          if (!err) onSave(q);
        }}
        fields={fields}
        values={values}
        onContext={onContext}
        invalid={!!err}
        end={
          <>
            <Button size="xs" variant="primary" disabled={!!err} onClick={() => onSave(text)}>
              {initial ? 'Save' : 'Add'}
            </Button>
            <Button size="xs" variant="quiet" onClick={onCancel}>
              Cancel
            </Button>
          </>
        }
      />
      {err && <span className="text-small text-danger">{err.message}</span>}
    </span>
  );
}

function RetrievalBar({
  state,
  vectorFields,
  vector,
  dim,
  onMode,
  onK,
  onFusion,
  onRrfK,
  onWeight,
  onVectorField,
  onVector,
}: {
  state: SearchState;
  vectorFields: FieldInfo[];
  vector: QueryVector | undefined;
  dim?: number;
  onMode: (m: Mode) => void;
  onK: (k: number) => void;
  onFusion: (f: FusionMethod) => void;
  onRrfK: (k: number) => void;
  onWeight: (w: number) => void;
  onVectorField: (f: string) => void;
  onVector: (v: QueryVector | undefined) => void;
}) {
  const hasVectors = vectorFields.length > 0;
  const fused = state.mode === 'hybrid' && !!vector;
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-md border border-rule bg-surface px-3 py-2 text-small">
      {hasVectors ? (
        <Segmented
          label="Retrieval mode"
          value={state.mode}
          onValueChange={onMode}
          options={[
            { value: 'hybrid', label: 'Hybrid', title: 'BM25 and vector, fused' },
            { value: 'text', label: 'Text', title: 'BM25 only' },
            { value: 'vector', label: 'Vector', title: 'Nearest neighbours only' },
          ]}
        />
      ) : (
        <span className="text-muted">Text search (this collection has no vectors)</span>
      )}
      {hasVectors && state.mode !== 'text' && (
        <Control label="Vector">
          {vectorFields.length > 1 && (
            <select
              className={SMALL_CONTROL}
              aria-label="Vector field"
              value={state.vectorField}
              onChange={(e) => onVectorField(e.target.value)}
            >
              {vectorFields.map((f) => (
                <option key={f.name}>{f.name}</option>
              ))}
            </select>
          )}
          {vector ? (
            <Badge variant="accent" size="lg">
              {vector.from === 'doc' ? (
                <>
                  Similar to <code>{vector.id}</code>
                </>
              ) : (
                `Pasted vector (${vector.values.length})`
              )}
              <button
                type="button"
                aria-label="Clear the vector"
                onClick={() => onVector(undefined)}
                className="ml-0.5 inline-flex cursor-pointer items-center border-0 bg-transparent p-0 text-inherit"
              >
                <X size={12} aria-hidden />
              </button>
            </Badge>
          ) : (
            <span className="text-muted">none: use Find similar on a result, or</span>
          )}
          <PasteVector dim={dim} onUse={(values) => onVector({ from: 'paste', values })} />
        </Control>
      )}
      <Control label="Top-k">
        <select
          className={SMALL_CONTROL}
          aria-label="Top-k"
          value={state.k}
          onChange={(e) => onK(Number(e.target.value))}
        >
          {[...new Set([...K_CHOICES, state.k])]
            .sort((a, b) => a - b)
            .map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
        </select>
      </Control>
      {hasVectors && state.mode === 'hybrid' && (
        <Control label="Fusion">
          <select
            className={SMALL_CONTROL}
            aria-label="Fusion method"
            value={state.fusion}
            onChange={(e) => onFusion(e.target.value as FusionMethod)}
            disabled={!fused}
          >
            <option value="rrf">Reciprocal rank (RRF)</option>
            <option value="weighted">Weighted sum</option>
          </select>
          {state.fusion === 'rrf' ? (
            <label className="inline-flex items-center gap-1 text-muted">
              k
              <input
                type="number"
                min={1}
                max={1000}
                className={cx(SMALL_CONTROL, 'w-16')}
                aria-label="RRF k"
                value={state.rrfK}
                disabled={!fused}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  if (Number.isInteger(n) && n >= 1) onRrfK(n);
                }}
              />
            </label>
          ) : (
            <label className="inline-flex items-center gap-2 text-muted">
              Text
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                aria-label="Text weight"
                aria-valuetext={`Text ${state.textWeight}, vector ${Math.round((1 - state.textWeight) * 100) / 100}`}
                value={state.textWeight}
                disabled={!fused}
                onChange={(e) => onWeight(Number(e.target.value))}
              />
              Vector
              <span className="text-ink tabular-nums">
                {state.textWeight.toFixed(2)} / {(1 - state.textWeight).toFixed(2)}
              </span>
            </label>
          )}
        </Control>
      )}
    </div>
  );
}

/** A compact select or number box for the retrieval bar. */
const SMALL_CONTROL =
  'h-7 rounded-sm border border-rule bg-bg px-2 text-small text-ink focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50';

function Control({ label, children }: { label: string; children: ReactNode }) {
  return (
    // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- a labelled group of inline controls
    <div role="group" aria-label={label} className="inline-flex flex-wrap items-center gap-1.5">
      <span className="text-muted">{label}</span>
      {children}
    </div>
  );
}

function PasteVector({ dim, onUse }: { dim?: number; onUse: (v: number[]) => void }) {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState('');
  const [err, setErr] = useState<string>();
  const id = useId();
  return (
    <Popover
      open={open}
      onOpenChange={setOpen}
      title="Paste a query vector"
      trigger={
        <Button size="xs" variant="quiet">
          Paste a vector
        </Button>
      }
    >
      <label htmlFor={id} className="text-small text-muted">
        A JSON array of {dim ?? 'N'} numbers
      </label>
      <textarea
        id={id}
        rows={5}
        className={/* mono-ok: code */ 'loams-input h-auto w-80 py-2 font-mono text-small'}
        value={text}
        aria-invalid={!!err || undefined}
        onChange={(e) => {
          setText(e.target.value);
          setErr(undefined);
        }}
        placeholder="[0.12, -0.03, …]"
      />
      {err && <span className="text-small text-danger">{err}</span>}
      <div className="flex justify-end gap-1">
        <Button
          size="sm"
          variant="primary"
          onClick={() => {
            try {
              onUse(parseVector(text, dim));
              setOpen(false);
            } catch (e) {
              setErr(errorText(e));
            }
          }}
        >
          Use vector
        </Button>
      </div>
    </Popover>
  );
}

function StatusLine({
  outcome,
  view,
  onView,
}: {
  outcome: Outcome;
  view: 'cards' | 'table';
  onView: (v: 'cards' | 'table') => void;
}) {
  return (
    <div
      className="flex flex-wrap items-center gap-x-3 gap-y-1 text-small text-muted"
      aria-live="polite"
    >
      {outcome.state === 'running' && <span>Searching…</span>}
      {outcome.state === 'done' && (
        <>
          <span className="text-body text-ink">
            <strong className="tabular-nums" data-testid="hit-count">
              {outcome.total === undefined
                ? outcome.hits.length.toLocaleString('en-US')
                : `${outcome.totalAtLeast ? '≥ ' : ''}${outcome.total.toLocaleString('en-US')}`}
            </strong>{' '}
            {outcome.total === 1 ? 'hit' : 'hits'}
            {outcome.total !== undefined && outcome.total > outcome.hits.length && (
              <span className="text-muted"> · showing {outcome.hits.length}</span>
            )}
          </span>
          <span className="tabular-nums">{outcome.ms} ms</span>
          <span>{outcome.plan}</span>
          {outcome.notes.map((n) => (
            <span key={n} className="text-warn">
              {n}
            </span>
          ))}
        </>
      )}
      <span className="ml-auto">
        <Segmented
          label="Result view"
          value={view}
          onValueChange={onView}
          options={[
            { value: 'cards', label: 'Cards', icon: <LayoutList size={13} aria-hidden /> },
            { value: 'table', label: 'Table', icon: <Table2 size={13} aria-hidden /> },
          ]}
        />
      </span>
    </div>
  );
}

function QueryHelp() {
  const rows: [string, string][] = [
    ['french river', 'words, ranked by BM25 over the text fields'],
    ['"french river"', 'a phrase'],
    ['title:river', 'words in one text field'],
    ['type:PushEvent', 'a keyword value; Push* is a prefix'],
    ['type:(A OR B)', 'one of several values'],
    ['body_chars >= 200', 'ranges: > >= < <='],
    ['created_at:2024-01-15', 'a whole day'],
    ['org:*', 'has a value'],
    ['a AND b, a OR b, NOT a', 'combine; parentheses group'],
  ];
  return (
    <Popover
      align="end"
      title="Query language"
      trigger={
        <Button size="sm" variant="quiet">
          Syntax
        </Button>
      }
    >
      <dl className="m-0 grid w-96 grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-small">
        {rows.map(([q, d]) => (
          <div key={q} className="contents">
            <dt>
              <code>{q}</code>
            </dt>
            <dd className="m-0 text-muted">{d}</dd>
          </div>
        ))}
      </dl>
      <p className="m-0 text-small text-muted">
        Words and text-field clauses rank the results; everything else filters them. Filters below
        the bar are ANDed with the query.
      </p>
    </Popover>
  );
}
