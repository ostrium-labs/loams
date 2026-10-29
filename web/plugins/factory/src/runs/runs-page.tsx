// The Runs page (design §72 §6.5, D1436; plan FT1, FT0 Task 9): Studio's list of factory
// runs, newest first, and the detail of the selected one: its stage, state, links (plan,
// threads, pull request, deploy), approvals and spend. Read-only: approvals are decided
// in Studio's approvals queue, and the kill switch is Task 10.

import { Badge, Card, DataTable, Empty, Notice } from '@loams/ui';
import { useEffect, useState } from 'react';
import type { RunRow, RunsSource } from './source.js';

/** How often the page reads the record again. */
export const POLL_MS = 2_000;

const STAGES = ['intent', 'plan', 'build', 'review', 'ci', 'merge', 'deploy', 'close'];

const STATE_VARIANT: Record<string, 'success' | 'warning' | 'danger' | 'neutral' | 'info'> = {
  open: 'info',
  waiting: 'warning',
  paused: 'warning',
  succeeded: 'success',
  failed: 'danger',
  killed: 'danger',
};

const when = (ms: number) => (ms ? new Date(ms).toLocaleString() : '—');
const money = (n: number | undefined) => (n === undefined ? '—' : `$${n.toFixed(2)}`);

function StatePill({ state }: { state: string }) {
  return <Badge variant={STATE_VARIANT[state] ?? 'neutral'}>{state}</Badge>;
}

function StageTrack({ stage }: { stage: string }) {
  const at = STAGES.indexOf(stage);
  return (
    <ol aria-label="Stages" className="flex flex-wrap gap-1 text-xs">
      {STAGES.map((s, i) => (
        <li
          key={s}
          aria-current={i === at ? 'step' : undefined}
          className={
            i === at
              ? 'rounded bg-accent-soft px-2 py-0.5 text-accent-fg'
              : i < at
                ? 'rounded px-2 py-0.5 text-muted'
                : 'rounded px-2 py-0.5 text-faint'
          }
        >
          {s}
        </li>
      ))}
    </ol>
  );
}

function Link({ href, children }: { href: string; children: string }) {
  return (
    <a href={href} target="_blank" rel="noreferrer" className="text-accent-fg underline">
      {children}
    </a>
  );
}

export function RunDetail({ run }: { run: RunRow }) {
  return (
    <section aria-label={`Run ${run.run}`} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <StatePill state={run.state} />
        <span className="text-sm text-muted">updated {when(run.updatedAt)}</span>
      </div>
      <StageTrack stage={run.stage} />
      {run.intent && <p className="text-sm">{run.intent}</p>}
      {run.error && (
        <Notice tone="danger" title="The run stopped">
          {run.error}
        </Notice>
      )}
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-muted">Plan</dt>
        <dd>{run.planUrl ? <Link href={run.planUrl}>plan.md</Link> : '—'}</dd>
        <dt className="text-muted">Threads</dt>
        <dd>{run.threads?.length ? run.threads.join(', ') : '—'}</dd>
        <dt className="text-muted">Pull request</dt>
        <dd>{run.pr ? <Link href={run.pr.url}>{`#${run.pr.number}`}</Link> : '—'}</dd>
        <dt className="text-muted">Checks</dt>
        <dd>{run.checks ?? '—'}</dd>
        <dt className="text-muted">Deploy</dt>
        <dd>
          {run.deploy ? `${run.deploy.app}: ${run.deploy.deploymentId} (${run.deploy.state})` : '—'}
        </dd>
        <dt className="text-muted">Spend</dt>
        <dd>{run.budget ? `${money(run.budget.spent)} of ${money(run.budget.limit)}` : '—'}</dd>
      </dl>
      <div>
        <h3 className="mb-1 text-sm font-medium">Approvals</h3>
        {run.approvals?.length ? (
          <ul className="flex flex-col gap-1 text-sm">
            {run.approvals.map((a) => (
              <li key={a.id}>
                {a.kind}: <StatePill state={a.state} />
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-faint">None yet</p>
        )}
      </div>
    </section>
  );
}

export function RunsPage({ source, pollMs = POLL_MS }: { source: RunsSource; pollMs?: number }) {
  const [runs, setRuns] = useState<RunRow[]>();
  const [error, setError] = useState<string>();
  const [selected, setSelected] = useState<string>();

  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        const rows = await source.list(abort.signal);
        if (abort.signal.aborted) return;
        setRuns(rows);
        setError(undefined);
      } catch (e) {
        if (abort.signal.aborted) return;
        setError(e instanceof Error ? e.message : String(e));
      }
      timer = setTimeout(() => void tick(), pollMs);
    };
    void tick();
    return () => {
      abort.abort();
      if (timer) clearTimeout(timer);
    };
  }, [source, pollMs]);

  const current = runs?.find((r) => r.run === selected) ?? runs?.[0];

  return (
    <div className="flex flex-col gap-4 p-6">
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h1 className="text-xl font-semibold">Runs</h1>
          <p className="text-sm text-muted">
            Factory runs from intent to deploy. <Badge variant="warning">experimental</Badge>
          </p>
        </div>
      </header>
      {error && (
        <Notice tone="danger" title="The run record could not be read">
          {error}. It lives in the Live app "factory".
        </Notice>
      )}
      {runs && runs.length === 0 && (
        <Empty title="No runs yet">Ask Loams Bot for a change to start one.</Empty>
      )}
      {runs && runs.length > 0 && (
        <div className="grid gap-4 lg:grid-cols-[3fr_2fr]">
          <Card title="Runs">
            <DataTable
              caption="Factory runs"
              rows={runs}
              rowKey={(r) => r.run}
              onRowClick={(r) => setSelected(r.run)}
              columns={[
                { key: 'run', header: 'Run', cell: (r) => r.run },
                { key: 'intent', header: 'Intent', cell: (r) => r.intent ?? '—' },
                { key: 'stage', header: 'Stage', cell: (r) => r.stage },
                { key: 'state', header: 'State', cell: (r) => <StatePill state={r.state} /> },
                { key: 'spend', header: 'Spend', cell: (r) => money(r.budget?.spent) },
                { key: 'updated', header: 'Updated', cell: (r) => when(r.updatedAt) },
              ]}
            />
          </Card>
          {current && (
            <Card title={`Run ${current.run}`}>
              <RunDetail run={current} />
            </Card>
          )}
        </div>
      )}
    </div>
  );
}
