/**
 * The React Flow tile.
 *
 * Modelled on `WidgetCard.tsx`: same card chrome (shared, via `cardChrome.tsx`),
 * same loading and error overlays, same `ResizeObserver`-driven resize handling,
 * same click-to-filter interaction. What differs is what is in the body.
 *
 * `WidgetCard` mounts ECharts imperatively -- `echarts.init`, `setOption`, and
 * a `click` listener re-attached on every render. This mounts React
 * declaratively, which changes three things worth stating:
 *
 * 1. There is no instance to dispose. React Flow's teardown is React's, so the
 *    `ResizeObserver` disconnect is the only explicit cleanup here, and getting
 *    that wrong leaks one observer per mount rather than one canvas.
 * 2. The click handler is a prop, not a subscription, so it cannot accumulate
 *    the way `instance.on("click", …)` did.
 * 3. The graph has to be re-fitted on resize. React Flow measures its own
 *    viewport, so a resized tile has to be told to re-fit or it keeps the zoom
 *    it had at mount and ends up half off-canvas.
 */
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  useReactFlow,
  type Node,
  type Edge,
  type NodeMouseHandler,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { cn } from "@loams-plugins/core/ui";
import { Widget, previewGraph } from "../api";
import {
  graphParamFilter,
  graphSubtitle,
  graphTitle,
  toFlowView,
  type FlowView,
} from "./graphView";
import { CardFrame, SPINNER, STATE_OVERLAY } from "./cardChrome";
import { NetworkIcon } from "./Icons";

interface GraphWidgetProps {
  widget: Widget;
  editMode: boolean;
  isSelected: boolean;
  params?: Record<string, unknown>;
  onSelect: (widget: Widget) => void;
  onDelete: (widgetId: string) => void;
  onParamChange?: (name: string, value: unknown) => void;
  /**
   * The dashboard-level theme selection, sent with the preview request.
   *
   * A graph widget cannot carry its own `flint.theme_spec` -- the schema
   * forbids it, because that is one of the ECharts inputs -- so this is always
   * the effective theme for the tile.
   */
  dashboardTheme?: unknown;
}

/** What the server resolved, minus the parts the tile supplies itself. */
interface PreviewResponse {
  nodes?: unknown;
  edges?: unknown;
  fitView?: unknown;
  pannable?: unknown;
  zoomable?: unknown;
}

const EMPTY_VIEW: FlowView = { nodes: [], edges: [], empty: true };

export const GraphWidget: React.FC<GraphWidgetProps> = ({
  widget,
  editMode,
  isSelected,
  params,
  onSelect,
  onDelete,
  onParamChange,
  dashboardTheme,
}) => {
  const [view, setView] = useState<FlowView>(EMPTY_VIEW);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [fitView, setFitView] = useState(true);
  const [pannable, setPannable] = useState(true);
  const [zoomable, setZoomable] = useState(true);
  // Bumped on resize. React Flow re-fits whenever this changes, which is how the
  // tile keeps a resized graph on-canvas without an imperative instance.
  const [fitToken, setFitToken] = useState(0);

  const containerRef = useRef<HTMLDivElement>(null);

  /**
   * A `loams` graph shows current state and re-reads; a Superset graph is a
   * warehouse scan over a period and would only get more expensive.
   */
  const live = widget.data.source === "loams";
  const LIVE_REFRESH_MS = 5_000;

  // Load the compiled graph.
  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);

    const load = () =>
      previewGraph(widget, params, dashboardTheme)
        .then((res: PreviewResponse) => {
          // Every branch below is guarded by `active`: this effect re-runs on
          // every params change, and a slow response for params that are no
          // longer current must not overwrite the graph now on screen.
          if (!active) return;
          setView(toFlowView(res));
          setFitView(res?.fitView !== false);
          setPannable(res?.pannable !== false);
          setZoomable(res?.zoomable !== false);
          // Cleared on success, so a recovered stream stops showing an error.
          setError(null);
          setLoading(false);
        })
        .catch((err: Error) => {
          if (!active) return;
          // An error clears the view rather than leaving the previous graph up:
          // a stale graph next to an error message reads as "this is what the
          // current filter produced", which is the opposite of what happened.
          setView(EMPTY_VIEW);
          setError(err?.message || "Failed to load the graph");
          setLoading(false);
        });

    load();

    let timer: ReturnType<typeof setInterval> | undefined;
    if (live) timer = setInterval(load, LIVE_REFRESH_MS);

    return () => {
      active = false;
      if (timer) clearInterval(timer);
    };
  }, [widget, JSON.stringify(params), dashboardTheme, live]);

  // Handle resize.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setFitToken((n) => n + 1));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // A node click drives cross-widget filtering, exactly as a series click does
  // on `WidgetCard`. The handler reads `widget.interactions` through the pure
  // `graphParamFilter`, so the mapping is testable without a DOM.
  const handleNodeClick = useCallback<NodeMouseHandler>(
    (_event, node) => {
      if (!onParamChange) return;
      for (const [name, value] of Object.entries(graphParamFilter(widget, node))) {
        onParamChange(name, value);
      }
    },
    [onParamChange, widget],
  );

  // React Flow's types are stricter than the repaired view: `Node` requires a
  // `position`, and `data` is a record. `toFlowView` guarantees both, which is
  // the whole reason it exists.
  const nodes = useMemo(() => view.nodes as unknown as Node[], [view.nodes]);
  const edges = useMemo(() => view.edges as unknown as Edge[], [view.edges]);

  const title = graphTitle(widget);
  const subtitle = graphSubtitle(widget);

  return (
    <CardFrame
      widgetId={widget.id}
      title={title}
      subtitle={subtitle}
      icon={<NetworkIcon />}
      editMode={editMode}
      isSelected={isSelected}
      onSelect={() => onSelect(widget)}
      onDelete={onDelete}
    >
      {loading && (
        <div className={STATE_OVERLAY} role="status" aria-label="Loading graph">
          <div className={SPINNER} />
        </div>
      )}
      {error && (
        <div className={STATE_OVERLAY} role="alert">
          <span className="text-[0.85rem] text-danger">{error}</span>
        </div>
      )}
      {!loading && !error && view.empty && (
        // Distinguishable from a failed render on purpose: both are a blank
        // canvas, and only one of them is the correct answer.
        <div className={STATE_OVERLAY}>
          <span className="text-[0.85rem] text-ink-body">This graph has no nodes to show.</span>
        </div>
      )}

      <ReactFlowProvider>
        <GraphCanvas
          containerRef={containerRef}
          nodes={nodes}
          edges={edges}
          fitView={fitView}
          fitToken={fitToken}
          pannable={pannable}
          zoomable={zoomable}
          visible={!loading && !error && !view.empty}
          onNodeClick={handleNodeClick}
        />
      </ReactFlowProvider>
    </CardFrame>
  );
};

/**
 * The React Flow instance, isolated so it can use the viewport hook.
 *
 * `useReactFlow` is only valid inside a `ReactFlowProvider`, and the hook cannot
 * be called in the component that renders the provider -- hence the split.
 */
const GraphCanvas: React.FC<{
  containerRef: React.RefObject<HTMLDivElement | null>;
  nodes: Node[];
  edges: Edge[];
  fitView: boolean;
  /** Changing this re-fits. See the resize note in `GraphWidget`. */
  fitToken: number;
  pannable: boolean;
  zoomable: boolean;
  visible: boolean;
  onNodeClick: NodeMouseHandler;
}> = ({
  containerRef,
  nodes,
  edges,
  fitView,
  fitToken,
  pannable,
  zoomable,
  visible,
  onNodeClick,
}) => (
  <div
    ref={containerRef}
    className={cn("h-full w-full flex-1")}
    style={{ opacity: visible ? 1 : 0 }}
  >
    <FitOnChange token={fitToken} enabled={fitView} />
    <ReactFlow
      nodes={nodes}
      edges={edges}
      fitView={fitView}
      fitViewOptions={{ padding: 0.15 }}
      /*
       * React Flow v12 dropped the old `pannable` / `zoomable` booleans for
       * three separate, more specific switches, so the spec's one flag each maps
       * onto two: zooming is enabled on the wheel AND on pinch, and panning on
       * a background drag. `preventScrolling` is left at its default -- turning
       * it on would swallow the page scroll inside a tile, which on a dashboard
       * grid is a worse problem than a tile that pans.
       */
      panOnDrag={pannable}
      zoomOnScroll={zoomable}
      zoomOnPinch={zoomable}
      onNodeClick={onNodeClick}
      nodesDraggable={false}
      nodesConnectable={false}
      elementsSelectable={false}
    >
      <Background variant={BackgroundVariant.Dots} gap={16} />
    </ReactFlow>
  </div>
);

/**
 * Re-fits the viewport when `token` changes.
 *
 * A child rather than a `useEffect` in `GraphCanvas` because `fitView()` comes
 * from `useReactFlow()`, which needs the provider above it -- and calling a hook
 * in the same component that renders the provider is the thing React forbids.
 */
const FitOnChange: React.FC<{ token: number; enabled: boolean }> = ({ token, enabled }) => {
  const { fitView } = useReactFlow();
  useEffect(() => {
    if (!enabled) return;
    // `fitView` returns a promise. It is not awaited here on purpose: inside an
    // effect a rejection would be an unhandled rejection, and React Flow only
    // rejects when the container has no measurable size yet -- which the next
    // resize corrects on its own.
    void Promise.resolve(fitView({ padding: 0.15 })).catch(() => {});
  }, [token, enabled, fitView]);
  return null;
};
