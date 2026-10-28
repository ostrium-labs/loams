// The Run graph tab: pick a run, watch it as a live DAG, open a step, and
// chart its step latencies and the run durations around it.

import { type ChartSpec, LiveChart, Sparkline, useMetricStream } from '@loams/charts';
import { useLoad } from '@loams/desktop-ui';
import {
  Button,
  Card,
  Empty,
  Field,
  Notice,
  SampleDataBadge,
  SampleDataNotice,
  Select,
  StatusTag,
} from '@loams/ui';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { EnvelopeError } from '../envelope.js';
import { ExecutionGraph, formatMs, STATUS_LABEL } from './ExecutionGraph.js';
import { applyEvent, isSettled, type RunGraph, runProgress, stepLatency } from './model.js';
import { StepPanel, statusTone } from './StepPanel.js';
import type { RunGraphSource } from './source.js';

export const LATENCY_SPEC: ChartSpec = {
  chartType: 'bar',
  title: 'Step latency',
  encodings: {
    x: { field: 'step', type: 'nominal' },
    y: { field: 'ms', type: 'quantitative' },
    color: { field: 'status', type: 'nominal' },
  },
};

export const DURATION_SPEC: ChartSpec = {
  chartType: 'line',
  title: 'Run duration',
  encodings: {
    x: { field: 't', type: 'temporal' },
    y: { field: 'v', type: 'quantitative' },
    color: { field: 'series', type: 'nominal' },
  },
};

/** The window of run durations the chart keeps. */
export const DURATION_WINDOW_MS = 10 * 60_000;

const errorText = (e: unknown) =>
  e instanceof EnvelopeError || e instanceof Error ? e.message : String(e);

export type StreamState = 'idle' | 'connecting' | 'live' | 'ended' | 'error';

/** Follows one run through `source`, folding its events into a graph. */
export function useRunStream(
  source: RunGraphSource,
  runId: string | undefined,
  replay = 0,
): { run: RunGraph | undefined; state: StreamState; error?: string } {
  const [run, setRun] = useState<RunGraph>();
  const [state, setState] = useState<StreamState>('idle');
  const [error, setError] = useState<string>();
  // oxlint-disable-next-line react/exhaustive-deps -- `replay` restarts the stream
  useEffect(() => {
    setRun(undefined);
    setError(undefined);
    if (!runId) {
      setState('idle');
      return;
    }
    setState('connecting');
    const ac = new AbortController();
    let current: RunGraph | undefined;
    (async () => {
      try {
        for await (const e of source.watchRun(runId, ac.signal)) {
          if (ac.signal.aborted) return;
          current = applyEvent(current, e);
          setRun(current);
          setState('live');
        }
        if (!ac.signal.aborted) setState('ended');
      } catch (e) {
        if (ac.signal.aborted) return;
        setError(errorText(e));
        setState('error');
      }
    })();
    return () => ac.abort();
  }, [source, runId, replay]);
  return { run, state, ...(error ? { error } : {}) };
}

/** How often a running step's latency bar is redrawn. */
export const LATENCY_TICK_MS = 1_000;

/** `Date.now()`, refreshed every `intervalMs` while it is set; frozen otherwise. */
export function useNow(intervalMs: number | undefined): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (intervalMs === undefined) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

export function GraphTab({ source }: { source: RunGraphSource }) {
  const [runs, reload] = useLoad(() => source.listRuns(), [source], { errorText });
  const [runId, setRunId] = useState<string>();
  const [replay, setReplay] = useState(0);
  const [selected, setSelected] = useState<string>();

  // Default to the first run once the list is in.
  useEffect(() => {
    if (runs.state === 'ready' && !runId && runs.data[0]) setRunId(runs.data[0].runId);
  }, [runs, runId]);

  const { run, state, error } = useRunStream(source, runId, replay);
  const step = run?.steps.find((s) => s.id === selected);

  const metrics = useCallback(() => source.metrics(), [source]);
  const durations = useMetricStream(metrics, DURATION_WINDOW_MS);
  const mine = useMemo(
    () => (run ? durations.filter((p) => p.series === run.name) : []),
    [durations, run],
  );

  // A running step's bar grows with the clock, not only when the run changes.
  const ticking =
    run?.steps.some((s) => s.startedAt !== undefined && !isSettled(s.status)) ?? false;
  const now = useNow(ticking ? LATENCY_TICK_MS : undefined);
  const latencies = useMemo(() => {
    if (!run) return [];
    return run.steps
      .filter((s) => s.kind !== 'workflow' && s.startedAt !== undefined)
      .map((s) => ({ step: s.name, ms: stepLatency(s, now) ?? 0, status: s.status }));
  }, [run, now]);

  const progress = run ? runProgress(run) : undefined;
  const summary = runs.state === 'ready' ? runs.data.find((r) => r.runId === runId) : undefined;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end gap-4">
        <div className="min-w-64">
          <Field label="Run">
            {(p) => (
              <Select
                {...p}
                value={runId ?? ''}
                disabled={runs.state !== 'ready' || runs.data.length === 0}
                onChange={(e) => {
                  setSelected(undefined);
                  setRunId(e.target.value || undefined);
                }}
              >
                {runs.state === 'ready' &&
                  runs.data.map((r) => (
                    <option key={r.runId} value={r.runId}>
                      {`${r.name} · ${r.runId}${r.sample ? ' (sample)' : ''}`}
                    </option>
                  ))}
              </Select>
            )}
          </Field>
        </div>
        <Button onClick={reload}>Refresh runs</Button>
        {summary?.sample && (
          <Button
            onClick={() => {
              setSelected(undefined);
              setReplay((n) => n + 1);
            }}
          >
            Replay sample
          </Button>
        )}
        {run && (
          <div className="flex items-center gap-2 ml-auto" aria-live="polite">
            <StatusTag status={statusTone(run.status)}>{STATUS_LABEL[run.status]}</StatusTag>
            <span className="text-muted text-body">
              {state === 'live' ? 'Live' : state === 'ended' ? 'Final' : ''}
            </span>
          </div>
        )}
      </div>

      {runs.state === 'loading' && <p className="text-muted text-body py-6">Loading runs…</p>}
      {runs.state === 'error' && (
        <Notice tone="danger" title="Could not load runs">
          {runs.message}
        </Notice>
      )}
      {runs.state === 'ready' && runs.data.length === 0 && (
        <Empty title="No runs">Start a workflow with a Resonate worker against this engine.</Empty>
      )}
      {state === 'error' && (
        <Notice tone="danger" title="Lost the run">
          {error}
        </Notice>
      )}
      {run?.sample && (
        <SampleDataNotice>
          This run is played from fixtures: the engine does not serve attempt history or a run
          change stream yet. Runs of this engine show what their promises record.
        </SampleDataNotice>
      )}

      {run && (
        <div className="flex flex-wrap gap-4 items-start">
          <div className="flex-1 min-w-0" style={{ minWidth: '20rem' }}>
            <Card
              title={run.name}
              actions={
                <span className="flex items-center gap-2">
                  {run.sample && <SampleDataBadge />}
                  {progress && (
                    <span className="text-muted text-body">
                      {progress.done} of {progress.total} steps settled
                    </span>
                  )}
                </span>
              }
            >
              <ExecutionGraph run={run} selectedId={selected} onSelect={setSelected} />
            </Card>
          </div>
          {step && (
            <div style={{ width: '22rem', maxWidth: '100%' }}>
              <StepPanel step={step} onClose={() => setSelected(undefined)} />
            </div>
          )}
        </div>
      )}

      {run && (
        <div className="flex flex-wrap gap-4">
          <div className="flex-1" style={{ minWidth: '18rem' }}>
            <Card title="Step latency">
              <LiveChart
                spec={LATENCY_SPEC}
                data={latencies}
                height={240}
                ariaLabel={`Latency of each step of ${run.name}, in milliseconds`}
              />
            </Card>
          </div>
          <div className="flex-1" style={{ minWidth: '18rem' }}>
            <Card
              title="Run duration"
              actions={
                mine.length > 0 && (
                  <span className="flex items-center gap-2 text-body text-muted">
                    <Sparkline
                      points={mine}
                      height={24}
                      ariaLabel={`Recent durations of ${run.name}`}
                    />
                    {formatMs(mine[mine.length - 1]?.v)}
                  </span>
                )
              }
            >
              <LiveChart
                spec={DURATION_SPEC}
                data={durations}
                windowMs={DURATION_WINDOW_MS}
                height={240}
                ariaLabel="Run duration in milliseconds over the last ten minutes, by workflow"
              />
            </Card>
          </div>
        </div>
      )}
    </div>
  );
}
