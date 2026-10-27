// Search hits as document cards or a table: highlighted text (the engine's highlights, or
// the query's words marked in the browser), the score and, for hybrid searches, each
// signal's rank; per hit: copy id, find similar, and the JSON.

import type { JsonObject, JsonValue } from '@bufbuild/protobuf';
import { Badge, Button, cx, StaggerItem, StaggerList } from '@loams/ui';
import { Braces, Check, Copy, Minus, Plus, Sparkles } from 'lucide-react';
import { Fragment, useState } from 'react';
import type { SearchHit } from '../client.js';
import type { FieldInfo } from './kql.js';
import { highlightRuns, markTerms } from './request.js';

export interface Ranks {
  text: Map<string, number>;
  vector: Map<string, number>;
}

export interface ResultsProps {
  hits: SearchHit[];
  fields: FieldInfo[];
  terms: string[];
  ranks?: Ranks;
  view: 'cards' | 'table';
  columns: string[];
  canSimilar: boolean;
  onSimilar: (hit: SearchHit) => void;
  onFilter: (field: string, value: string, negate: boolean) => void;
}

function show(v: JsonValue | undefined): string {
  if (v === undefined || v === null) return '';
  return typeof v === 'string' ? v : JSON.stringify(v);
}

function Runs({ runs }: { runs: { text: string; hit: boolean }[] }) {
  return (
    <>
      {runs.map((r, i) =>
        r.hit ? (
          // oxlint-disable-next-line react/no-array-index-key -- positional runs of one string
          <mark key={i} className="rounded-xs bg-warn-soft px-0.5 text-ink">
            {r.text}
          </mark>
        ) : (
          // oxlint-disable-next-line react/no-array-index-key -- positional runs of one string
          <Fragment key={i}>{r.text}</Fragment>
        ),
      )}
    </>
  );
}

/**
 * A field's text with the query marked: the engine's fragments when it sent some, else
 * the value with the query's own words marked in the browser.
 */
function Highlighted({
  hit,
  field,
  terms,
  max,
}: {
  hit: SearchHit;
  field: string;
  terms: string[];
  max: number;
}) {
  const frags = hit.highlight[field];
  if (frags && frags.length > 0) {
    return (
      <>
        {frags.map((f, i) => (
          // oxlint-disable-next-line react/no-array-index-key -- fragments in engine order
          <Fragment key={i}>
            {i > 0 && <span className="text-faint"> … </span>}
            <Runs runs={highlightRuns(f)} />
          </Fragment>
        ))}
      </>
    );
  }
  const text = show(hit.source[field]);
  const cut = text.length > max ? `${text.slice(0, max - 1)}…` : text;
  return <Runs runs={markTerms(cut, terms)} />;
}

export function Results(p: ResultsProps) {
  return p.view === 'cards' ? <Cards {...p} /> : <ResultTable {...p} />;
}

function Cards({ hits, fields, terms, ranks, canSimilar, onSimilar, onFilter }: ResultsProps) {
  const textFields = fields.filter((f) => f.type === 'text').map((f) => f.name);
  return (
    <StaggerList
      as="ol"
      className="m-0 flex list-none flex-col gap-2 p-0"
      aria-label="Search results"
    >
      {hits.map((h, i) => {
        const titleField = textFields.find((f) => {
          const v = h.source[f];
          return typeof v === 'string' && v.trim() !== '';
        });
        const bodyFields = textFields.filter((f) => f !== titleField && h.source[f] !== undefined);
        const order = fields.map((f) => f.name);
        const rank = (k: string) => (order.includes(k) ? order.indexOf(k) : order.length);
        const other = Object.keys(h.source)
          .filter((k) => !textFields.includes(k))
          .sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
        return (
          <StaggerItem key={h.id} index={i}>
            <article
              aria-labelledby={`hit-${i}`}
              className="flex flex-col gap-2 rounded-md border border-rule bg-surface px-4 py-3"
            >
              <header className="flex items-start gap-3">
                <span className="mt-0.5 w-6 shrink-0 text-right text-small text-faint tabular-nums">
                  {i + 1}
                </span>
                <h3
                  id={`hit-${i}`}
                  className="m-0 min-w-0 flex-1 text-emphasis font-semibold text-ink"
                >
                  {titleField ? (
                    <Highlighted hit={h} field={titleField} terms={terms} max={200} />
                  ) : (
                    <code>{h.id}</code>
                  )}
                </h3>
                <Scores hit={h} ranks={ranks} />
              </header>
              {bodyFields.map((f) => (
                <p key={f} className="m-0 ml-9 text-body leading-relaxed text-muted">
                  {bodyFields.length > 1 && (
                    <span className="mr-1.5 text-small text-faint">{f}</span>
                  )}
                  <Highlighted hit={h} field={f} terms={terms} max={320} />
                </p>
              ))}
              <HitFooter
                hit={h}
                other={other}
                terms={terms}
                canSimilar={canSimilar}
                onSimilar={onSimilar}
                onFilter={onFilter}
              />
            </article>
          </StaggerItem>
        );
      })}
    </StaggerList>
  );
}

function Scores({ hit, ranks }: { hit: SearchHit; ranks?: Ranks }) {
  const t = ranks?.text.get(hit.id);
  const v = ranks?.vector.get(hit.id);
  return (
    <div className="flex shrink-0 flex-wrap items-center justify-end gap-1">
      {ranks && (
        <>
          <Badge variant={t ? 'info' : 'neutral'} title="Rank in the BM25 (text) list on its own">
            BM25 {t ? `#${t}` : '–'}
          </Badge>
          <Badge variant={v ? 'accent' : 'neutral'} title="Rank in the vector list on its own">
            Vector {v ? `#${v}` : '–'}
          </Badge>
        </>
      )}
      {hit.score !== 0 && (
        <Badge variant="outline" title="The score the engine ranked by">
          <span data-testid="score">{hit.score.toFixed(4)}</span>
        </Badge>
      )}
    </div>
  );
}

function HitFooter({
  hit,
  other,
  terms,
  canSimilar,
  onSimilar,
  onFilter,
}: {
  hit: SearchHit;
  other: string[];
  terms: string[];
  canSimilar: boolean;
  onSimilar: (h: SearchHit) => void;
  onFilter: (field: string, value: string, negate: boolean) => void;
}) {
  const [json, setJson] = useState(false);
  return (
    <>
      {other.length > 0 && (
        <dl className="m-0 ml-9 flex flex-wrap gap-1.5">
          {other.map((k) => {
            const v = hit.source[k];
            const text = show(v);
            const scalar = v !== null && typeof v !== 'object';
            return (
              <div
                key={k}
                className="group/f inline-flex max-w-full min-w-0 items-center gap-1 rounded-sm bg-raised px-1.5 py-0.5 text-small"
              >
                <dt className="text-muted">{k}</dt>
                <dd className="m-0 min-w-0 truncate text-ink" title={text}>
                  {v === null ? (
                    <span className="text-faint">null</span>
                  ) : (
                    <Runs
                      runs={markTerms(text.length > 80 ? `${text.slice(0, 79)}…` : text, terms)}
                    />
                  )}
                </dd>
                {scalar && (
                  <span className="hidden gap-0.5 group-focus-within/f:inline-flex group-hover/f:inline-flex">
                    <MiniButton
                      label={`Filter for ${k} ${text}`}
                      onClick={() => onFilter(k, text, false)}
                    >
                      <Plus size={11} aria-hidden />
                    </MiniButton>
                    <MiniButton
                      label={`Filter out ${k} ${text}`}
                      onClick={() => onFilter(k, text, true)}
                    >
                      <Minus size={11} aria-hidden />
                    </MiniButton>
                  </span>
                )}
              </div>
            );
          })}
        </dl>
      )}
      <div className="ml-9 flex flex-wrap items-center gap-1">
        <span className="mr-1 text-small text-faint">
          id <code className="text-muted">{hit.id}</code>
        </span>
        <CopyId id={hit.id} />
        {canSimilar && (
          <Button size="xs" variant="quiet" onClick={() => onSimilar(hit)}>
            <Sparkles size={12} aria-hidden /> Find similar
          </Button>
        )}
        <Button size="xs" variant="quiet" aria-expanded={json} onClick={() => setJson(!json)}>
          <Braces size={12} aria-hidden /> {json ? 'Hide JSON' : 'JSON'}
        </Button>
      </div>
      {json && (
        <pre className="ds-json m-0 ml-9 rounded-sm">
          {JSON.stringify({ id: hit.id, score: hit.score, source: hit.source }, null, 2)}
        </pre>
      )}
    </>
  );
}

function MiniButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="inline-flex size-4 cursor-pointer items-center justify-center rounded-xs border-0 bg-bg p-0 text-muted hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
    >
      {children}
    </button>
  );
}

function CopyId({ id }: { id: string }) {
  const [done, setDone] = useState(false);
  return (
    <Button
      size="xs"
      variant="quiet"
      aria-label={`Copy id ${id}`}
      onClick={() => {
        void globalThis.navigator?.clipboard?.writeText(id).then(
          () => {
            setDone(true);
            setTimeout(() => setDone(false), 1500);
          },
          () => {},
        );
      }}
    >
      {done ? <Check size={12} aria-hidden /> : <Copy size={12} aria-hidden />}{' '}
      {done ? 'Copied' : 'Copy id'}
    </Button>
  );
}

function ResultTable({ hits, fields, terms, ranks, columns, canSimilar, onSimilar }: ResultsProps) {
  const [open, setOpen] = useState<string>();
  const cols =
    columns.length > 0
      ? columns
      : fields
          .filter((f) => f.type !== 'vector')
          .slice(0, 4)
          .map((f) => f.name);
  return (
    <div className="loams-table-wrap rounded-md border border-rule">
      <table className="loams-table" aria-label="Search results">
        <thead>
          <tr>
            <th scope="col">#</th>
            <th scope="col">id</th>
            <th scope="col" className="text-right">
              Score
            </th>
            {ranks && <th scope="col">Ranks</th>}
            {cols.map((c) => (
              <th key={c} scope="col">
                {c}
              </th>
            ))}
            <th scope="col">
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {hits.map((h, i) => (
            <Fragment key={h.id}>
              <tr>
                <td className="text-faint tabular-nums">{i + 1}</td>
                <td>
                  <code>{h.id}</code>
                </td>
                <td className="text-right tabular-nums">{h.score ? h.score.toFixed(4) : ''}</td>
                {ranks && (
                  <td className="whitespace-nowrap text-small text-muted">
                    BM25 {ranks.text.get(h.id) ?? '–'} · Vector {ranks.vector.get(h.id) ?? '–'}
                  </td>
                )}
                {cols.map((c) => (
                  <td key={c} className="max-w-80">
                    <span className="ds-cell max-w-80">
                      <Highlighted hit={h} field={c} terms={terms} max={120} />
                    </span>
                  </td>
                ))}
                <td className="whitespace-nowrap">
                  <CopyId id={h.id} />
                  {canSimilar && (
                    <Button
                      size="xs"
                      variant="quiet"
                      aria-label={`Find documents similar to ${h.id}`}
                      onClick={() => onSimilar(h)}
                    >
                      <Sparkles size={12} aria-hidden />
                    </Button>
                  )}
                  <Button
                    size="xs"
                    variant="quiet"
                    aria-expanded={open === h.id}
                    aria-label={`JSON of ${h.id}`}
                    onClick={() => setOpen(open === h.id ? undefined : h.id)}
                  >
                    <Braces size={12} aria-hidden />
                  </Button>
                </td>
              </tr>
              {open === h.id && (
                <tr>
                  <td colSpan={cols.length + (ranks ? 5 : 4)} className={cx('p-0')}>
                    <pre className="ds-json m-0">
                      {JSON.stringify(
                        { id: h.id, score: h.score, source: h.source as JsonObject },
                        null,
                        2,
                      )}
                    </pre>
                  </td>
                </tr>
              )}
            </Fragment>
          ))}
        </tbody>
      </table>
    </div>
  );
}
