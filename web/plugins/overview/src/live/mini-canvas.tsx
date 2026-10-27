// A compact, read-only picture of the project's services: a small React Flow
// graph, self-contained (it shares no code with the canvas plugin). Nothing
// can be dragged, connected, panned or zoomed. Each service is a focusable
// node: Tab reaches the first, the arrow keys move between them by position,
// Home and End jump to the first and last. A visually hidden list says the
// same thing in text for screen readers.

import {
  Background,
  BackgroundVariant,
  type Edge,
  Handle,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { Card, Notice } from '@loams/ui';
import { type KeyboardEvent, useCallback, useMemo, useRef } from 'react';
import { useLoad } from '../load.js';
import type { OverviewMetricsClient, ServiceHealth, ServiceNode, Topology } from './types.js';

const COL_W = 170;
const ROW_H = 72;

const HEALTH: Record<ServiceHealth, { label: string; color: string }> = {
  healthy: { label: 'Healthy', color: 'var(--op-grow)' },
  degraded: { label: 'Degraded', color: 'var(--op-accent)' },
  down: { label: 'Down', color: 'var(--op-danger)' },
};

type ServiceData = { service: ServiceNode };
type ServiceFlowNode = Node<ServiceData, 'service'>;

/** True when the user asked for less motion; false where `matchMedia` is missing. */
export function reducedMotion(): boolean {
  try {
    return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
  } catch {
    return false;
  }
}

export function serviceLabel(s: ServiceNode): string {
  return `${s.name}, ${s.kind}, ${HEALTH[s.health].label}`;
}

function ServiceNodeView({ data }: NodeProps<ServiceFlowNode>) {
  const s = data.service;
  const h = HEALTH[s.health];
  return (
    <div
      className="flex items-center gap-2 px-2 py-1 text-small"
      style={{
        background: 'var(--op-raised)',
        color: 'var(--op-ink)',
        border: '1px solid var(--op-rule)',
        minWidth: 120,
      }}
    >
      <Handle type="target" position={Position.Left} isConnectable={false} />
      <span
        aria-hidden="true"
        style={{ width: 8, height: 8, borderRadius: 999, background: h.color, flex: 'none' }}
      />
      <span className="flex flex-col">
        <strong>{s.name}</strong>
        <span style={{ color: 'var(--op-muted)' }}>
          {s.kind} · {h.label}
        </span>
      </span>
      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
}

const nodeTypes = { service: ServiceNodeView };

export function toFlow(
  topo: Topology,
  animate: boolean,
): { nodes: ServiceFlowNode[]; edges: Edge[] } {
  const nodes: ServiceFlowNode[] = topo.services.map((s) => ({
    id: s.id,
    type: 'service',
    position: { x: s.col * COL_W, y: s.row * ROW_H },
    data: { service: s },
    ariaLabel: serviceLabel(s),
    draggable: false,
    connectable: false,
    selectable: false,
    focusable: true,
  }));
  const known = new Set(topo.services.map((s) => s.id));
  const edges: Edge[] = topo.links
    .filter((l) => known.has(l.from) && known.has(l.to))
    .map((l) => ({
      id: `${l.from}->${l.to}`,
      source: l.from,
      target: l.to,
      // Motion only for those who have not asked for less.
      animated: animate,
      focusable: false,
      selectable: false,
      style: { stroke: 'var(--op-muted)' },
    }));
  return { nodes, edges };
}

/**
 * The service the arrow key moves to from `from`: the nearest one strictly in
 * that direction, by grid distance (the cross axis weighs double so the move
 * stays in its lane where it can).
 */
export function nextService(
  services: readonly ServiceNode[],
  from: string,
  key: string,
): string | undefined {
  const order = [...services].sort((a, b) => a.col - b.col || a.row - b.row);
  if (key === 'Home') return order[0]?.id;
  if (key === 'End') return order[order.length - 1]?.id;
  const cur = services.find((s) => s.id === from);
  if (!cur) return order[0]?.id;
  const dir: Record<string, [number, number]> = {
    ArrowRight: [1, 0],
    ArrowLeft: [-1, 0],
    ArrowDown: [0, 1],
    ArrowUp: [0, -1],
  };
  const d = dir[key];
  if (!d) return undefined;
  let best: { id: string; score: number } | undefined;
  for (const s of services) {
    const dc = s.col - cur.col;
    const dr = s.row - cur.row;
    const along = dc * d[0] + dr * d[1];
    if (along <= 0) continue;
    const across = Math.abs(d[0] !== 0 ? dr : dc);
    const score = along + 2 * across;
    if (!best || score < best.score) best = { id: s.id, score };
  }
  return best?.id;
}

function Graph({ topo }: { topo: Topology }) {
  const animate = !reducedMotion();
  const { nodes, edges } = useMemo(() => toFlow(topo, animate), [topo, animate]);
  const ref = useRef<HTMLDivElement>(null);
  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>('.react-flow__node');
      const from = el?.dataset.id;
      if (!from) return;
      const to = nextService(topo.services, from, e.key);
      if (!to) return;
      const target = ref.current?.querySelector<HTMLElement>(
        `.react-flow__node[data-id="${CSS.escape(to)}"]`,
      );
      if (!target) return;
      e.preventDefault();
      e.stopPropagation();
      target.focus();
    },
    [topo.services],
  );
  return (
    <>
      {/* oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- a React Flow viewport holding focusable nodes, not a set of form controls; the group names it and delegates arrow keys */}
      <div
        ref={ref}
        role="group"
        aria-label="Service map. Use the arrow keys to move between services."
        style={{ height: 240 }}
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
          elementsSelectable={false}
          panOnDrag={false}
          panOnScroll={false}
          zoomOnScroll={false}
          zoomOnPinch={false}
          zoomOnDoubleClick={false}
          preventScrolling={false}
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Dots} gap={16} size={1} color="var(--op-rule)" />
        </ReactFlow>
      </div>
      <ul className="sr-only" aria-label="Services and their connections">
        {topo.services.map((s) => {
          const to = topo.links
            .filter((l) => l.from === s.id)
            .map((l) => topo.services.find((x) => x.id === l.to)?.name)
            .filter(Boolean);
          return (
            <li key={s.id}>
              {serviceLabel(s)}
              {to.length > 0 ? `; calls ${to.join(', ')}` : ''}
            </li>
          );
        })}
      </ul>
    </>
  );
}

export function MiniCanvas({ client }: { client: OverviewMetricsClient }) {
  const load = useLoad(() => client.topology(), client);
  return (
    <div data-panel="canvas">
      <Card title="Services" flush>
        {load.state === 'loading' && <p className="text-body text-muted m-3">Loading</p>}
        {load.state === 'error' && (
          <Notice tone="danger" title="Not reachable">
            {load.message}
          </Notice>
        )}
        {load.state === 'ok' && (
          <ReactFlowProvider>
            <Graph topo={load.data} />
          </ReactFlowProvider>
        )}
      </Card>
    </div>
  );
}
