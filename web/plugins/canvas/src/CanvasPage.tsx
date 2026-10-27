// The project canvas: a project's services and the dependencies and data flows between
// them, Railway-style, on React Flow. Nodes carry live status and a metric sparkline;
// positions persist per project through the client; a node opens a details panel.

import { PageHead } from '@loams/desktop-ui';
import { Badge, Button, Notice, SampleDataNotice } from '@loams/ui';
import {
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  MarkerType,
  MiniMap,
  type NodeChange,
  type NodeMouseHandler,
  type OnNodeDrag,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import './canvas.css';
import {
  type KeyboardEvent,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react';
import { AddServiceDialog, type AddServiceRequest } from './AddService.js';
import {
  type CanvasClient,
  errorText,
  type Position,
  type Project,
  type Service,
  type ServiceEdge,
} from './client.js';
import { layeredLayout, nextFreePosition } from './layout.js';
import { ARROW_DIRECTIONS, nearestInDirection, readingOrder } from './navigate.js';
import { CanvasContext, nodeLabel, type ServiceFlowNode, ServiceNode } from './ServiceNode.js';
import { SidePanel } from './SidePanel.js';

/**
 * Opening and re-laying-out fit the services, but never below 0.75 (UX-6): at 1440 ten
 * services fitted at once put node text near 6px. A wider project opens readable around
 * its middle, and the minimap and zoom controls reach the rest. Never above 1 either, so
 * a project of two services does not open at twice its size.
 */
export const FIT_VIEW = { padding: 0.15, minZoom: 0.75, maxZoom: 1 } as const;

const NODE_TYPES = { service: ServiceNode };
/** How far Shift+arrow nudges a focused node. */
export const NUDGE = 24;

/** Whether the viewer asked for less motion; false where matchMedia is missing. */
export function useReducedMotion(): boolean {
  const query = '(prefers-reduced-motion: reduce)';
  const read = () => typeof window !== 'undefined' && !!window.matchMedia?.(query).matches;
  const [reduced, setReduced] = useState(read);
  useEffect(() => {
    const mq = window.matchMedia?.(query);
    if (!mq) return;
    const on = () => setReduced(mq.matches);
    mq.addEventListener?.('change', on);
    return () => mq.removeEventListener?.('change', on);
  }, []);
  return reduced;
}

/**
 * Where every service starts: its saved position, else the layered layout when nothing is
 * saved, else a free spot below what is already placed.
 */
export function initialPositions(p: Project): Record<string, Position> {
  const saved = p.services.filter((s) => p.positions[s.id]);
  if (saved.length === 0) {
    return layeredLayout(
      p.services.map((s) => s.id),
      p.edges,
    );
  }
  const out: Record<string, Position> = {};
  for (const s of saved) out[s.id] = p.positions[s.id] as Position;
  for (const s of p.services) {
    if (!out[s.id]) out[s.id] = nextFreePosition(Object.values(out));
  }
  return out;
}

export function toFlowEdges(edges: readonly ServiceEdge[], reducedMotion: boolean): Edge[] {
  return edges.map((e) => ({
    id: e.id,
    source: e.source,
    target: e.target,
    type: 'smoothstep',
    label: e.label,
    animated: e.kind === 'data-flow' && !reducedMotion,
    className: e.kind === 'data-flow' ? 'lc-edge-flow' : 'lc-edge-dep',
    markerEnd: { type: MarkerType.ArrowClosed },
    focusable: false,
    selectable: false,
  }));
}

function toNode(s: Service, position: Position, selected: boolean): ServiceFlowNode {
  return {
    id: s.id,
    type: 'service',
    position,
    data: { service: s },
    selected,
    ariaLabel: nodeLabel(s),
  };
}

export function CanvasPage({
  client,
  projectId,
  navigate,
  sample,
}: {
  client: CanvasClient;
  projectId: string;
  navigate: (to: string) => void;
  /** The client is a fixture: the page says so. */
  sample?: boolean;
}) {
  const [project, setProject] = useState<Project>();
  const [failure, setFailure] = useState<string>();
  const [attempt, setAttempt] = useState(0);

  // oxlint-disable-next-line react/exhaustive-deps -- `attempt` retries on purpose
  useEffect(() => {
    const abort = new AbortController();
    setProject(undefined);
    setFailure(undefined);
    client.getProject(projectId, abort.signal).then(
      (p) => !abort.signal.aborted && setProject(p),
      (e) => !abort.signal.aborted && setFailure(errorText(e)),
    );
    return () => abort.abort();
  }, [client, projectId, attempt]);

  // Live status: patch each service's status as updates arrive. Restarts only when the
  // project loads, not on every status change.
  const loaded = project?.id === projectId;
  useEffect(() => {
    if (!loaded) return;
    const abort = new AbortController();
    (async () => {
      try {
        for await (const batch of client.watchStatus(projectId, abort.signal)) {
          if (abort.signal.aborted) break;
          const next = new Map(batch.map((u) => [u.serviceId, u.status]));
          setProject(
            (p) =>
              p && {
                ...p,
                services: p.services.map((s) => {
                  const st = next.get(s.id);
                  return st && st !== s.status ? { ...s, status: st } : s;
                }),
              },
          );
        }
      } catch {
        // The badges keep their last status; a reload starts the watch again.
      }
    })();
    return () => abort.abort();
  }, [client, projectId, loaded]);

  const head = (
    <PageHead
      title="Canvas"
      badge={project ? <Badge>{project.name}</Badge> : undefined}
      subtitle="Your project's services and how data moves between them."
    />
  );

  if (failure) {
    return (
      <div className="lc-page">
        {head}
        <Notice tone="danger" title="Could not load the project">
          {failure}
        </Notice>
        <div>
          <Button onClick={() => setAttempt((n) => n + 1)}>Try again</Button>
        </div>
      </div>
    );
  }
  if (!project) {
    return (
      <div className="lc-page">
        {head}
        <p className="text-muted" role="status">
          Loading the project…
        </p>
      </div>
    );
  }
  return (
    <div className="lc-page">
      {head}
      {sample && (
        <SampleDataNotice>
          Services you add or move here are not saved to a project.
        </SampleDataNotice>
      )}
      <CanvasContext.Provider value={{ client, projectId }}>
        <ReactFlowProvider>
          <ProjectCanvas
            client={client}
            project={project}
            onProject={setProject}
            navigate={navigate}
          />
        </ReactFlowProvider>
      </CanvasContext.Provider>
    </div>
  );
}

function ProjectCanvas({
  client,
  project,
  onProject,
  navigate,
}: {
  client: CanvasClient;
  project: Project;
  onProject: (update: (p: Project | undefined) => Project | undefined) => void;
  navigate: (to: string) => void;
}) {
  const { fitView } = useReactFlow();
  const reducedMotion = useReducedMotion();
  const wrapper = useRef<HTMLElement>(null);
  const hintId = useId();
  const [nodes, setNodes] = useState<ServiceFlowNode[]>(() => {
    const pos = initialPositions(project);
    return project.services.map((s) => toNode(s, pos[s.id] as Position, false));
  });
  const [selectedId, setSelectedId] = useState<string>();
  const [adding, setAdding] = useState(false);
  const [saveFailure, setSaveFailure] = useState<string>();
  const [announcement, setAnnouncement] = useState('');

  // Keep node data in step with the project (status changes, new services), keeping
  // each node's current position.
  useEffect(() => {
    setNodes((prev) => {
      const byId = new Map(prev.map((n) => [n.id, n]));
      const placed = prev.map((n) => n.position);
      return project.services.map((s) => {
        const was = byId.get(s.id);
        let position = was?.position ?? project.positions[s.id];
        if (!position) {
          position = nextFreePosition(placed);
          placed.push(position);
        }
        const next = toNode(s, position, s.id === selectedId);
        // Keep what React Flow measured, so a status tick does not re-measure every node.
        return was ? { ...was, ...next } : next;
      });
    });
  }, [project, selectedId]);

  const edges = useMemo(
    () => toFlowEdges(project.edges, reducedMotion),
    [project.edges, reducedMotion],
  );
  const selected = project.services.find((s) => s.id === selectedId);

  const save = useCallback(
    (positions: Record<string, Position>) => {
      client.savePositions(project.id, positions).then(
        () => setSaveFailure(undefined),
        (e) => setSaveFailure(errorText(e)),
      );
    },
    [client, project.id],
  );

  const onNodesChange = useCallback((changes: NodeChange<ServiceFlowNode>[]) => {
    setNodes((nds) => applyNodeChanges(changes, nds));
  }, []);

  const onNodeDragStop = useCallback<OnNodeDrag<ServiceFlowNode>>(
    (_e, _node, dragged) => {
      const out: Record<string, Position> = {};
      for (const n of dragged) out[n.id] = n.position;
      save(out);
    },
    [save],
  );

  const onNodeClick = useCallback<NodeMouseHandler<ServiceFlowNode>>((_e, node) => {
    setSelectedId(node.id);
  }, []);

  const focusNode = useCallback((id: string) => {
    const all = wrapper.current?.querySelectorAll<HTMLElement>('.react-flow__node') ?? [];
    [...all].find((n) => n.dataset.id === id)?.focus();
  }, []);

  const autoLayout = () => {
    const pos = layeredLayout(
      project.services.map((s) => s.id),
      project.edges,
    );
    setNodes((nds) => nds.map((n) => ({ ...n, position: pos[n.id] ?? n.position })));
    save(pos);
    setAnnouncement('Services laid out again.');
    // After React Flow has the new positions.
    requestAnimationFrame(() => void fitView({ ...FIT_VIEW, duration: reducedMotion ? 0 : 300 }));
  };

  const create = async (req: AddServiceRequest) => {
    const { service, edges: added } = await client.createService(project.id, req);
    const position = nextFreePosition(nodes.map((n) => n.position));
    // The sync effect places the node at `position` from the project's saved positions.
    onProject(
      (p) =>
        p && {
          ...p,
          services: [...p.services, service],
          edges: [...p.edges, ...added],
          positions: { ...p.positions, [service.id]: position },
        },
    );
    save({ [service.id]: position });
    setAdding(false);
    setSelectedId(service.id);
    setAnnouncement(`Added ${nodeLabel(service)}.`);
  };

  const closePanel = useCallback(() => {
    const was = selectedId;
    setSelectedId(undefined);
    if (was) requestAnimationFrame(() => focusNode(was));
  }, [selectedId, focusNode]);

  // Keyboard: arrows move between nodes, Shift+arrows move the node, Enter/Space open it,
  // Home/End jump to the first/last node, Escape closes the panel. Captured before React
  // Flow's own handler, which would otherwise move the selected node on a bare arrow.
  const onKeyDownCapture = (e: KeyboardEvent<HTMLDivElement>) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>('.react-flow__node');
    const id = el?.dataset.id;
    if (!id) return;
    const placed = nodes.map((n) => ({ id: n.id, position: n.position }));
    const dir = ARROW_DIRECTIONS[e.key];
    if (dir && !e.altKey && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      e.stopPropagation();
      if (e.shiftKey) {
        const n = nodes.find((x) => x.id === id);
        if (!n) return;
        const dx = dir === 'left' ? -NUDGE : dir === 'right' ? NUDGE : 0;
        const dy = dir === 'up' ? -NUDGE : dir === 'down' ? NUDGE : 0;
        const position = { x: n.position.x + dx, y: n.position.y + dy };
        setNodes((nds) => nds.map((x) => (x.id === id ? { ...x, position } : x)));
        save({ [id]: position });
        return;
      }
      const next = nearestInDirection(placed, id, dir);
      if (next) focusNode(next);
      return;
    }
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      e.stopPropagation();
      setSelectedId(id);
      return;
    }
    if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      const order = readingOrder(placed);
      const target = e.key === 'Home' ? order[0] : order.at(-1);
      if (target) focusNode(target);
      return;
    }
    if (e.key === 'Escape' && selectedId) {
      e.stopPropagation();
      closePanel();
    }
  };

  return (
    <>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={() => setAdding(true)}>
          Add service
        </Button>
        <Button onClick={autoLayout}>Auto-layout</Button>
        <span className="text-sm text-muted">
          {project.services.length} services · {project.edges.length} connections
        </span>
      </div>
      {saveFailure && (
        <div className="mb-3">
          <Notice tone="danger" title="Could not save the layout">
            {saveFailure}
          </Notice>
        </div>
      )}
      <div className="flex flex-col gap-4 xl:flex-row xl:items-start">
        <section
          ref={wrapper}
          className="lc-canvas relative min-w-0 flex-1 border border-rule"
          style={{ height: 'min(70vh, 720px)', minHeight: 420 }}
          onKeyDownCapture={onKeyDownCapture}
          aria-label={`${project.name} services canvas`}
          aria-describedby={hintId}
        >
          <ReactFlow<ServiceFlowNode, Edge>
            nodes={nodes}
            edges={edges}
            nodeTypes={NODE_TYPES}
            onNodesChange={onNodesChange}
            onNodeDragStop={onNodeDragStop}
            onNodeClick={onNodeClick}
            onPaneClick={() => setSelectedId(undefined)}
            nodesConnectable={false}
            // Services are removed on their product page, never by a stray Backspace here.
            deleteKeyCode={null}
            nodesFocusable
            edgesFocusable={false}
            elementsSelectable
            fitView
            fitViewOptions={FIT_VIEW}
            minZoom={0.25}
            maxZoom={2}
            proOptions={{ hideAttribution: true }}
          >
            <Background variant={BackgroundVariant.Dots} gap={20} size={1} />
            <MiniMap
              pannable
              zoomable
              ariaLabel="Canvas overview"
              nodeClassName={(n) =>
                `lc-mini-${(n as ServiceFlowNode).data?.service.status ?? 'stopped'}`
              }
            />
            <Controls showInteractive={false} aria-label="Zoom controls" />
          </ReactFlow>
        </section>
        {selected && (
          <SidePanel
            service={selected}
            services={project.services}
            edges={project.edges}
            onClose={closePanel}
            onNavigate={navigate}
            onFocusService={setSelectedId}
          />
        )}
      </div>
      <p id={hintId} className="mt-2 text-xs text-muted">
        Tab to a service, then use the arrow keys to move between services, Shift with an arrow to
        move the service, and Enter to open its details.
      </p>
      <div className="sr-only" role="status" aria-live="polite">
        {announcement}
      </div>
      <AddServiceDialog
        open={adding}
        services={project.services}
        onClose={() => setAdding(false)}
        onCreate={create}
      />
    </>
  );
}
