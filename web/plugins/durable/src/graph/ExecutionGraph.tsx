// The run DAG on a React Flow canvas (as plugins/core/dashboard-ui's
// GraphWidget does it): one node per step, coloured by status, retries drawn
// as a stack of attempt cards, timers and awaits as their own shapes.
//
// Keyboard: Tab reaches the nodes; the arrow keys move between them along the
// graph (left/right follow edges, up/down walk a column); Enter or Space opens
// the step. The step list under the canvas is the same graph as plain buttons,
// for screen readers and anyone who would rather not use the canvas.

import {
  Background,
  BackgroundVariant,
  type Edge,
  Handle,
  MarkerType,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { Clock, GitBranch, Hourglass, Network, Play } from 'lucide-react';
import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import './graph.css';
import { type Direction, layoutRun, neighbour } from './layout.js';
import type { RunGraph, Step, StepKind, StepStatus } from './model.js';

export const STATUS_LABEL: Record<StepStatus, string> = {
  pending: 'pending',
  running: 'running',
  succeeded: 'succeeded',
  failed: 'failed',
  retrying: 'retrying',
  sleeping: 'sleeping',
};

const KIND_LABEL: Record<StepKind, string> = {
  workflow: 'workflow',
  step: 'step',
  rpc: 'remote call',
  timer: 'timer',
  await: 'await',
};

const KIND_ICON: Record<StepKind, typeof Play> = {
  workflow: GitBranch,
  step: Play,
  rpc: Network,
  timer: Clock,
  await: Hourglass,
};

const time = (ms: number | undefined) => (ms ? new Date(ms).toLocaleTimeString() : '');

export function formatMs(ms: number | undefined): string {
  if (ms === undefined) return '—';
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)} m ${Math.round((ms % 60_000) / 1000)} s`;
}

/** What a screen reader hears for a step. */
export function describeStep(s: Step): string {
  const parts = [s.name, KIND_LABEL[s.kind], STATUS_LABEL[s.status]];
  if (s.attempts.length > 1) parts.push(`attempt ${s.attempts.length}`);
  if (s.kind === 'timer' && s.status === 'sleeping' && s.wakeAt)
    parts.push(`wakes at ${time(s.wakeAt)}`);
  if (s.endedAt !== undefined && s.startedAt !== undefined)
    parts.push(`took ${formatMs(s.endedAt - s.startedAt)}`);
  return parts.join(', ');
}

/** `prefers-reduced-motion: reduce`, tracked. */
export function usePrefersReducedMotion(): boolean {
  const query = '(prefers-reduced-motion: reduce)';
  const get = () =>
    typeof window !== 'undefined' && typeof window.matchMedia === 'function'
      ? window.matchMedia(query).matches
      : false;
  const [reduced, setReduced] = useState(get);
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;
    const m = window.matchMedia(query);
    const on = () => setReduced(m.matches);
    m.addEventListener?.('change', on);
    return () => m.removeEventListener?.('change', on);
  }, []);
  return reduced;
}

type StepData = { step: Step; chosen: boolean };
type StepNodeT = Node<StepData, 'step'>;

function StepNode({ data }: NodeProps<StepNodeT>) {
  const { step, chosen } = data;
  const Icon = KIND_ICON[step.kind];
  const stack = Math.min(step.attempts.length, 3);
  return (
    <div
      className="dg-node"
      data-kind={step.kind}
      data-status={step.status}
      data-stack={stack > 1 ? stack : undefined}
      data-chosen={chosen || undefined}
    >
      <Handle type="target" position={Position.Left} isConnectable={false} />
      <div className="dg-node-head">
        <Icon size={14} aria-hidden="true" />
        <span className="dg-node-name">{step.name}</span>
      </div>
      <div className="dg-node-foot">
        <span className="dg-dot" aria-hidden="true" />
        <span>{STATUS_LABEL[step.status]}</span>
        {step.attempts.length > 1 && <span className="dg-attempts">×{step.attempts.length}</span>}
        {step.kind === 'timer' && step.status === 'sleeping' && step.wakeAt ? (
          <span className="dg-meta">wakes {time(step.wakeAt)}</span>
        ) : step.endedAt !== undefined && step.startedAt !== undefined ? (
          <span className="dg-meta">{formatMs(step.endedAt - step.startedAt)}</span>
        ) : null}
      </div>
      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
}

const nodeTypes = { step: StepNode };

export interface ExecutionGraphProps {
  run: RunGraph;
  selectedId?: string | undefined;
  onSelect: (id: string) => void;
  height?: number;
}

function Canvas({ run, selectedId, onSelect, height = 420 }: ExecutionGraphProps) {
  const reduced = usePrefersReducedMotion();
  const wrap = useRef<HTMLElement>(null);
  const rf = useReactFlow();
  const layout = useMemo(() => layoutRun(run), [run]);
  const byId = useMemo(() => new Map(run.steps.map((s) => [s.id, s])), [run]);

  const nodes = useMemo<StepNodeT[]>(
    () =>
      run.steps.map((s) => {
        const p = layout.placed.get(s.id);
        return {
          id: s.id,
          type: 'step',
          position: { x: p?.x ?? 0, y: p?.y ?? 0 },
          data: { step: s, chosen: s.id === selectedId },
          selected: s.id === selectedId,
          ariaLabel: describeStep(s),
          draggable: false,
          connectable: false,
        };
      }),
    [run, layout, selectedId],
  );

  const edges = useMemo<Edge[]>(
    () =>
      layout.edges.map(([a, b]) => {
        const to = byId.get(b);
        const live = to?.status === 'running' || to?.status === 'retrying';
        return {
          id: `${a}->${b}`,
          source: a,
          target: b,
          animated: live && !reduced,
          className: `dg-edge dg-edge-${to?.status ?? 'pending'}`,
          markerEnd: { type: MarkerType.ArrowClosed },
          focusable: false,
        };
      }),
    [layout, byId, reduced],
  );

  // Re-fit when the shape of the graph changes (not on every status tick).
  const shape = run.steps.map((s) => s.id).join('|');
  // oxlint-disable-next-line react/exhaustive-deps -- refit on a new shape only
  useEffect(() => {
    rf.fitView({ padding: 0.15, duration: reduced ? 0 : 200 });
  }, [shape, reduced]);

  const focusNode = useCallback(
    (id: string) => {
      const el = wrap.current?.querySelector<HTMLElement>(
        `.react-flow__node[data-id="${CSS.escape(id)}"]`,
      );
      el?.focus();
      const p = layout.placed.get(id);
      if (p) rf.setCenter(p.x + 90, p.y + 30, { zoom: rf.getZoom(), duration: reduced ? 0 : 150 });
    },
    [layout, rf, reduced],
  );

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>('.react-flow__node');
    const id = el?.getAttribute('data-id');
    if (!id) return;
    const dirs: Record<string, Direction> = {
      ArrowLeft: 'left',
      ArrowRight: 'right',
      ArrowUp: 'up',
      ArrowDown: 'down',
    };
    const dir = dirs[e.key];
    if (dir) {
      e.preventDefault();
      e.stopPropagation();
      const next = neighbour(layout, id, dir);
      if (next !== id) focusNode(next);
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onSelect(id);
    }
  };

  return (
    <section
      ref={wrap}
      className="dg-canvas"
      style={{ height }}
      aria-roledescription="execution graph"
      aria-label={`Execution graph of ${run.name}: ${run.steps.length} steps. Arrow keys move between steps, Enter opens one.`}
      onKeyDown={onKeyDown}
    >
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.15 }}
        nodesDraggable={false}
        nodesConnectable={false}
        nodesFocusable
        edgesFocusable={false}
        elementsSelectable
        minZoom={0.3}
        maxZoom={1.6}
        proOptions={{ hideAttribution: true }}
        onNodeClick={(_, n) => onSelect(n.id)}
      >
        <Background variant={BackgroundVariant.Dots} gap={18} size={1} />
      </ReactFlow>
    </section>
  );
}

export function ExecutionGraph(props: ExecutionGraphProps) {
  return (
    <div className="dg-root">
      <ReactFlowProvider>
        <Canvas {...props} />
      </ReactFlowProvider>
      <StepList run={props.run} selectedId={props.selectedId} onSelect={props.onSelect} />
    </div>
  );
}

/** The graph as a list of buttons, in column order. */
export function StepList({
  run,
  selectedId,
  onSelect,
}: {
  run: RunGraph;
  selectedId?: string | undefined;
  onSelect: (id: string) => void;
}) {
  const layout = useMemo(() => layoutRun(run), [run]);
  const byId = new Map(run.steps.map((s) => [s.id, s]));
  return (
    <ol className="dg-list" aria-label={`Steps of ${run.name}`}>
      {layout.columns.map((col, c) =>
        col.map((id) => {
          const s = byId.get(id);
          if (!s) return null;
          return (
            <li key={id} data-col={c}>
              <button
                type="button"
                className="dg-chip"
                data-status={s.status}
                aria-current={id === selectedId ? 'true' : undefined}
                aria-label={describeStep(s)}
                onClick={() => onSelect(id)}
              >
                <span className="dg-dot" aria-hidden="true" />
                {s.name}
                {s.attempts.length > 1 && (
                  <span className="dg-attempts" aria-hidden="true">
                    ×{s.attempts.length}
                  </span>
                )}
              </button>
            </li>
          );
        }),
      )}
    </ol>
  );
}
