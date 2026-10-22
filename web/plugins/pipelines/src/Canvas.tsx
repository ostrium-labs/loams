// The React Flow canvas (@xyflow/react 12, as @loams-core/dashboard-ui mounts it). The
// builder graph is the source of truth; React Flow's nodes and edges are derived from it
// on every render and its changes are folded back (moves, selection, deletes).
//
// Accessibility: React Flow makes every node focusable (Tab), Enter or Space selects
// it, arrow keys move a selected node, and Delete or Backspace removes it. Each node
// names itself (kind, label, problems), each handle is labelled, and an error edge
// carries its problems in its aria-label. The outline list beside the canvas
// (Outline.tsx) is the non-spatial path to the same graph, including connecting nodes.
// Motion: running edges animate only without `prefers-reduced-motion`, and fitView
// does not tween then.

import { Sparkline, useMetricStream } from '@loams/charts';
import { formatNumber, prefersReducedMotion } from '@loams/ui';
import {
  Background,
  BackgroundVariant,
  BaseEdge,
  type Connection,
  Controls,
  type Edge,
  type EdgeChange,
  EdgeLabelRenderer,
  type EdgeProps,
  getSmoothStepPath,
  Handle,
  MarkerType,
  type Node,
  type NodeChange,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { type CSSProperties, type DragEvent, memo, useCallback, useMemo } from 'react';
import type { MetricPoint, PipelinesClient } from './client.js';
import {
  FAMILY_LABEL,
  type PipelineGraph,
  type PipelineNode,
  TRANSFORM_LABEL,
  type TransformKind,
} from './model.js';
import { DRAG_MIME, decodeItem, type PaletteItem } from './Palette.js';
import { canConnect, type EdgeCheck, type Lookup } from './schema.js';

/** What a running node needs to draw its live series. */
export interface Live {
  client: PipelinesClient;
  deploymentId: string;
}

type PNodeData = {
  node: PipelineNode;
  subtitle: string;
  schema: string;
  problems: readonly string[];
  live?: Live;
};
type PNode = Node<PNodeData, 'pipeline'>;

type TEdgeData = { check?: EdgeCheck };
type TEdge = Edge<TEdgeData, 'typed'>;

const KIND_LABEL = { source: 'Source', sink: 'Sink', transform: 'Transform' } as const;
const KIND_TONE = {
  source: 'border-l-grow',
  transform: 'border-l-info',
  sink: 'border-l-accent',
} as const;

export const NODE_WINDOW_MS = 60_000;

/** "1.2k ev/s · lag 0.8 s": the newest point of each series. */
export function latestText(
  throughput: readonly MetricPoint[],
  lag: readonly MetricPoint[],
): string {
  const t = throughput.at(-1)?.v;
  const l = lag.at(-1)?.v;
  const parts: string[] = [];
  if (t !== undefined) parts.push(`${formatNumber(Math.round(t))} ev/s`);
  if (l !== undefined) parts.push(`lag ${l.toFixed(1)} s`);
  return parts.join(' · ') || 'waiting for metrics';
}

function NodeMetricsMini({ live, nodeId, label }: { live: Live; nodeId: string; label: string }) {
  const { client, deploymentId } = live;
  const throughputSource = useCallback(
    () => client.metrics(deploymentId, nodeId, 'throughput'),
    [client, deploymentId, nodeId],
  );
  const lagSource = useCallback(
    () => client.metrics(deploymentId, nodeId, 'lag'),
    [client, deploymentId, nodeId],
  );
  const throughput = useMetricStream(throughputSource, NODE_WINDOW_MS);
  const lag = useMetricStream(lagSource, NODE_WINDOW_MS);
  return (
    <div className="mt-1 flex flex-col gap-1">
      <Sparkline points={throughput} height={28} ariaLabel={`${label} throughput, last minute`} />
      <span className="text-small text-muted tabular-nums" aria-live="off">
        {latestText(throughput, lag)}
      </span>
    </div>
  );
}

const PipelineNodeView = memo(function PipelineNodeView({ data, selected }: NodeProps<PNode>) {
  const { node, subtitle, schema, problems, live } = data;
  const name = `${KIND_LABEL[node.kind]} ${node.label}, ${subtitle}${
    problems.length ? `, ${problems.length} problem${problems.length > 1 ? 's' : ''}` : ''
  }`;
  return (
    <fieldset
      aria-label={name}
      className={`m-0 w-[200px] min-w-0 border border-l-4 border-solid bg-surface p-2 text-ink ${KIND_TONE[node.kind]} ${
        selected
          ? 'border-accent shadow-[0_0_0_2px_var(--op-accent)]'
          : problems.length
            ? 'border-danger'
            : 'border-rule'
      }`}
    >
      {node.kind !== 'source' && (
        <Handle type="target" position={Position.Left} aria-label={`${node.label} input`} />
      )}
      <div className="flex items-center justify-between gap-2">
        <span className="text-micro font-medium text-muted">{KIND_LABEL[node.kind]}</span>
        {problems.length > 0 && (
          <span className="text-[10px] font-medium text-danger" title={problems.join('\n')}>
            {problems.length} issue{problems.length > 1 ? 's' : ''}
          </span>
        )}
      </div>
      <div className="truncate text-body font-medium">{node.label}</div>
      <div className="truncate text-small text-faint">{subtitle}</div>
      <div
        className={/* mono-ok: code */ 'truncate font-mono text-[10px] text-muted'}
        title={schema}
      >
        {schema}
      </div>
      {live && <NodeMetricsMini live={live} nodeId={node.id} label={node.label} />}
      {node.kind !== 'sink' && (
        <Handle type="source" position={Position.Right} aria-label={`${node.label} output`} />
      )}
    </fieldset>
  );
});

function TypedEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  selected,
  data,
}: EdgeProps<TEdge>) {
  const [path, labelX, labelY] = getSmoothStepPath({
    sourceX,
    sourceY,
    targetX,
    targetY,
    sourcePosition,
    targetPosition,
  });
  const check = data?.check;
  const error = check && !check.ok;
  const style: CSSProperties = {
    stroke: error ? 'var(--op-danger)' : selected ? 'var(--op-accent)' : 'var(--op-muted)',
    strokeWidth: selected ? 2.5 : 1.5,
    ...(error ? { strokeDasharray: '6 4' } : {}),
  };
  return (
    <>
      <BaseEdge id={id} path={path} markerEnd={markerEnd} style={style} />
      {error && (
        <EdgeLabelRenderer>
          <div
            className="nodrag nopan absolute border border-solid border-danger bg-danger-soft px-1.5 py-0.5 text-[10px] font-medium text-danger"
            style={{
              transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`,
              pointerEvents: 'all',
            }}
            title={check.problems.join('\n')}
          >
            {check.message}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}

const nodeTypes = { pipeline: PipelineNodeView };
const edgeTypes = { typed: TypedEdge };

export interface CanvasProps {
  graph: PipelineGraph;
  lookup: Lookup;
  schemas: ReadonlyMap<string, string>;
  edgeChecks: ReadonlyMap<string, EdgeCheck>;
  problemsByNode: ReadonlyMap<string, readonly string[]>;
  selectedId?: string;
  live?: Live;
  readOnly?: boolean;
  onSelect: (id: string | undefined) => void;
  onMove: (id: string, position: { x: number; y: number }) => void;
  onConnect: (source: string, target: string) => void;
  onRemoveNodes: (ids: string[]) => void;
  onRemoveEdges: (ids: string[]) => void;
  onDropItem: (item: PaletteItem, position: { x: number; y: number }) => void;
}

function subtitleFor(node: PipelineNode, lookup: Lookup): string {
  if (node.kind === 'transform') return TRANSFORM_LABEL[node.ref as TransformKind] ?? node.ref;
  const def = lookup(node.ref);
  return def ? `${def.name} · ${FAMILY_LABEL[def.family]}` : node.ref;
}

function CanvasInner(props: CanvasProps) {
  const { graph, lookup, schemas, edgeChecks, problemsByNode, selectedId, live, readOnly } = props;
  const { screenToFlowPosition } = useReactFlow();
  const reduced = useMemo(() => {
    try {
      return prefersReducedMotion();
    } catch {
      return true;
    }
  }, []);

  const nodes: PNode[] = useMemo(
    () =>
      graph.nodes.map((n) => ({
        id: n.id,
        type: 'pipeline' as const,
        position: n.position,
        selected: n.id === selectedId,
        draggable: !readOnly,
        deletable: !readOnly,
        data: {
          node: n,
          subtitle: subtitleFor(n, lookup),
          schema: schemas.get(n.id) ?? '',
          problems: problemsByNode.get(n.id) ?? [],
          ...(live ? { live } : {}),
        },
      })),
    [graph.nodes, selectedId, readOnly, lookup, schemas, problemsByNode, live],
  );

  const edges: TEdge[] = useMemo(
    () =>
      graph.edges.map((e) => {
        const check = edgeChecks.get(e.id);
        const from = graph.nodes.find((n) => n.id === e.source)?.label ?? e.source;
        const to = graph.nodes.find((n) => n.id === e.target)?.label ?? e.target;
        return {
          id: e.id,
          source: e.source,
          target: e.target,
          type: 'typed' as const,
          deletable: !readOnly,
          animated: Boolean(live) && !reduced && check?.ok !== false,
          markerEnd: { type: MarkerType.ArrowClosed },
          ariaLabel:
            check && !check.ok
              ? `Connection ${from} to ${to}: ${check.problems.join(' ')}`
              : `Connection ${from} to ${to}`,
          data: { check },
        };
      }),
    [graph.edges, graph.nodes, edgeChecks, live, reduced, readOnly],
  );

  const onNodesChange = useCallback(
    (changes: NodeChange<PNode>[]) => {
      const removed: string[] = [];
      for (const c of changes) {
        if (c.type === 'position' && c.position && !readOnly) props.onMove(c.id, c.position);
        else if (c.type === 'select' && c.selected) props.onSelect(c.id);
        else if (c.type === 'remove' && !readOnly) removed.push(c.id);
      }
      if (removed.length) props.onRemoveNodes(removed);
    },
    [props, readOnly],
  );

  const onEdgesChange = useCallback(
    (changes: EdgeChange<TEdge>[]) => {
      if (readOnly) return;
      const removed = changes.filter((c) => c.type === 'remove').map((c) => c.id);
      if (removed.length) props.onRemoveEdges(removed);
    },
    [props, readOnly],
  );

  const isValidConnection = useCallback(
    (c: Connection | Edge) => !readOnly && canConnect(graph, c.source, c.target).ok,
    [graph, readOnly],
  );

  const onDragOver = (e: DragEvent) => {
    if (readOnly || !e.dataTransfer.types.includes(DRAG_MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  };
  const onDrop = (e: DragEvent) => {
    if (readOnly) return;
    const item = decodeItem(e.dataTransfer.getData(DRAG_MIME));
    if (!item) return;
    e.preventDefault();
    props.onDropItem(item, screenToFlowPosition({ x: e.clientX, y: e.clientY }));
  };

  // React Flow's own chrome, on the console's tokens, so light and dark follow the theme.
  const themed = {
    '--xy-background-color': 'var(--op-bg)',
    '--xy-background-pattern-color': 'var(--op-rule)',
    '--xy-edge-stroke': 'var(--op-muted)',
    '--xy-handle-background-color': 'var(--op-accent)',
    '--xy-handle-border-color': 'var(--op-bg)',
    '--xy-controls-button-background-color': 'var(--op-surface)',
    '--xy-controls-button-background-color-hover': 'var(--op-raised)',
    '--xy-controls-button-color': 'var(--op-ink)',
    '--xy-controls-button-border-color': 'var(--op-rule)',
    '--xy-connectionline-stroke': 'var(--op-accent)',
    '--xy-selection-background-color': 'var(--op-accent-soft)',
    '--xy-selection-border': '1px dashed var(--op-accent)',
  } as CSSProperties;

  return (
    <div className="h-full min-h-[420px] w-full border border-solid border-rule" style={themed}>
      <ReactFlow<PNode, TEdge>
        onDragOver={onDragOver}
        onDrop={onDrop}
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={(c) => !readOnly && props.onConnect(c.source, c.target)}
        isValidConnection={isValidConnection}
        onPaneClick={() => props.onSelect(undefined)}
        nodesConnectable={!readOnly}
        nodesFocusable
        edgesFocusable
        deleteKeyCode={readOnly ? null : ['Backspace', 'Delete']}
        fitView
        fitViewOptions={{ padding: 0.2, duration: reduced ? 0 : 200 }}
        minZoom={0.25}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        aria-label="Pipeline canvas"
      >
        <Background variant={BackgroundVariant.Dots} gap={20} size={1} />
        <Controls showInteractive={false} aria-label="Canvas zoom controls" />
      </ReactFlow>
    </div>
  );
}

export function Canvas(props: CanvasProps) {
  return (
    <ReactFlowProvider>
      <CanvasInner {...props} />
    </ReactFlowProvider>
  );
}
