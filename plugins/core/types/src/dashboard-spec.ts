import { z } from "zod";
import { ThemeSelectionSchema } from "./theme-spec.js";

export const InteractionSchema = z.object({
  on: z.enum(["click", "brush"]),
  set: z.record(z.string(), z.string()),
});
export type Interaction = z.infer<typeof InteractionSchema>;

export const ChartSpecSchema = z.object({
  chartType: z.string(),
  encodings: z.record(
    z.string(),
    z.union([
      z.string(),
      z.object({
        field: z.string(),
        type: z.string().optional(),
        aggregate: z.string().optional(),
      }),
    ]),
  ),
  title: z.string().optional(),
  subtitle: z.string().optional(),
  baseSize: z
    .object({
      width: z.number(),
      height: z.number(),
    })
    .optional(),
  chartProperties: z.record(z.string(), z.unknown()).optional(),
  theme_spec: z.union([z.string(), z.record(z.string(), z.unknown())]).optional(),
});
export type ChartSpec = z.infer<typeof ChartSpecSchema>;

export const ChartSchema = z.object({
  kind: z.enum(["line", "bar", "pie", "scatter", "heatmap", "funnel", "sankey", "area", "custom"]),
  encode: z.object({
    x: z.union([z.string(), z.array(z.string())]).optional(),
    y: z.union([z.string(), z.array(z.string())]).optional(),
    series: z.union([z.string(), z.array(z.string())]).optional(),
    value: z.union([z.string(), z.array(z.string())]).optional(),
  }),
  optionOverrides: z.record(z.string(), z.unknown()).optional(),
});
export type Chart = z.infer<typeof ChartSchema>;

/**
 * Which Loams stream a live widget reads.
 *
 * Every one of these is a server-streaming `Watch*` RPC, so the source is a
 * cursor-resumable stream rather than a table: a dashboard tile fed from one of
 * these is a projection of "what is happening now", not a historical scan.
 */
export const LoamsStreamSchema = z.enum(["operations", "approvals", "notifications"]);
export type LoamsStream = z.infer<typeof LoamsStreamSchema>;

export const BiDataSourceSchema = z.object({
  source: z.literal("bi"),
  datasetId: z.number().nullable().optional(),
  sql: z.string().nullable().optional(),
  params: z.array(z.string()).optional(),
});

/**
 * Loams' own live state, rather than an analytics warehouse.
 *
 * `stream` picks the watch RPC; `token` is the bearer the server presents, and
 * `filter` narrows the projection (an approval tile scoped to one state, an
 * operations tile scoped to one project). The stream is resumed from a cursor
 * on reconnect, so a dropped connection replays rather than restarts.
 */
export const LoamsDataSourceSchema = z.object({
  source: z.literal("loams"),
  stream: LoamsStreamSchema,
  /** Defaults to the mock's read token, `mock-access-usr_omar`. */
  token: z.string().min(1).optional(),
  filter: z.record(z.string(), z.unknown()).optional(),
});
export type LoamsDataSource = z.infer<typeof LoamsDataSourceSchema>;

export const DataSourceSchema = z.discriminatedUnion("source", [
  BiDataSourceSchema,
  LoamsDataSourceSchema,
]);
export type DataSource = z.infer<typeof DataSourceSchema>;

/**
 * Which way a graph's ranks flow.
 *
 * Named after the four directions a layered layout can run. `LR` (left to
 * right) is the default because a service or pipeline topology reads naturally
 * as a row, and it is the direction that stays legible at dashboard-tile width.
 */
export const GraphDirectionSchema = z.enum(["TB", "LR", "BT", "RL"]);
export type GraphDirection = z.infer<typeof GraphDirectionSchema>;

export const GraphNodeSchema = z.object({
  /** The key React Flow mounts on. Unique within a graph -- see below. */
  id: z.string().min(1),
  /**
   * A literal label. Wins over `labelField` and over the node id, which is what
   * makes a hand-authored graph read as prose rather than as a list of ids.
   */
  label: z.string().optional(),
  /**
   * A column to read the label from, when the label lives in the data rather
   * than in the spec. Looked up on the row whose `id` equals this node's.
   */
  labelField: z.string().min(1).optional(),
  /**
   * An explicit position, bypassing the computed layout.
   *
   * Deliberately `finite()`: React Flow positions a node with these numbers
   * directly, and a `NaN` handed to a transform produces a canvas that renders
   * nothing with no error anywhere.
   */
  position: z
    .object({
      x: z.number().finite(),
      y: z.number().finite(),
    })
    .optional(),
  /** Overrides the palette-derived fill for this node only. */
  color: z.string().min(1).optional(),
  className: z.string().min(1).optional(),
  width: z.number().positive().optional(),
  height: z.number().positive().optional(),
});
export type GraphNode = z.infer<typeof GraphNodeSchema>;

export const GraphEdgeSchema = z.object({
  /**
   * Optional here, but never optional downstream: the compiler mints a stable
   * one. Two edges between the same pair are a parallel edge, so the derived id
   * carries the edge's index and cannot collide with itself.
   */
  id: z.string().min(1).optional(),
  source: z.string().min(1),
  target: z.string().min(1),
  label: z.string().optional(),
  value: z.number().finite().optional(),
  animated: z.boolean().optional(),
  color: z.string().min(1).optional(),
  type: z.enum(["default", "straight", "smoothstep", "simplebezier"]).optional(),
});
export type GraphEdge = z.infer<typeof GraphEdgeSchema>;

/**
 * Layered-layout parameters.
 *
 * Every field has a default, so the compiler never has to ask whether one is
 * present. `rankSep` is the gap between depth levels and `nodeSep` the gap
 * between siblings at the same depth; they are separate because a wide fan-out
 * needs to breathe horizontally while staying tight vertically.
 */
export const GraphLayoutSchema = z.object({
  direction: GraphDirectionSchema.default("LR"),
  nodeWidth: z.number().positive().default(180),
  nodeHeight: z.number().positive().default(48),
  rankSep: z.number().min(0).default(80),
  nodeSep: z.number().min(0).default(32),
});
export type GraphLayout = z.infer<typeof GraphLayoutSchema>;

const DEFAULT_GRAPH_LAYOUT_INPUT = {
  direction: "LR",
  nodeWidth: 180,
  nodeHeight: 48,
  rankSep: 80,
  nodeSep: 32,
} as const;

/**
 * Draw the graph from the data instead of from a hand-written node list.
 *
 * A literal `nodes` array can only name ids the author already knows. That is the
 * right shape for a fixed pipeline and the wrong one for a live stream, where the
 * operations arriving are not known until they arrive -- so a hand-authored list
 * either goes stale or has to be rebuilt on every tick, which is authoring logic
 * in the wrong place. Naming columns instead lets the same spec describe the
 * topology as the rows describe the data.
 *
 * Every field is a column name, read off each row. `id` is what React Flow keys
 * on, so it has to come from the data rather than the row index.
 */
export const GraphDeriveSchema = z.object({
  /** The column holding the node id. Required: the id cannot be synthesised. */
  idField: z.string().min(1),
  /** The column to label the node with. Falls back to the id. */
  labelField: z.string().min(1).optional(),
  /**
   * The column holding the parent/source id, and the one holding the child/target
   * id. Together they make one edge per row.
   *
   * A row whose source names no node in the graph is dropped rather than
   * mounted: React Flow renders such an edge as nothing, so the graph comes up
   * quietly missing a connection.
   */
  sourceField: z.string().min(1).optional(),
  targetField: z.string().min(1).optional(),
  /** An edge label column, when the relationship has a name worth showing. */
  edgeLabelField: z.string().min(1).optional(),
  /**
   * Colour nodes by the value of this column.
   *
   * One colour per distinct value, cycled over the palette, so a stream that
   * mixes states reads as such without the spec enumerating them.
   */
  colorField: z.string().min(1).optional(),
});
export type GraphDerive = z.infer<typeof GraphDeriveSchema>;

export const GraphSpecSchema = z
  .object({
    /**
     * At least one, unless `derive` supplies the nodes from the data instead.
     *
     * A graph with neither is not an empty state worth rendering -- it is an
     * authoring mistake, and failing at parse time says so instead of leaving a
     * blank tile on the dashboard.
     */
    nodes: z.array(GraphNodeSchema).optional(),
    derive: GraphDeriveSchema.optional(),
    edges: z.array(GraphEdgeSchema).default([]),
    layout: GraphLayoutSchema.default(DEFAULT_GRAPH_LAYOUT_INPUT),
    fitView: z.boolean().default(true),
    pannable: z.boolean().default(true),
    zoomable: z.boolean().default(true),
  })
  .superRefine((spec, ctx) => {
    const nodes = spec.nodes ?? [];
    const deriving = spec.derive !== undefined;

    // Either name the nodes or say where to read them from. Neither means the
    // graph has nothing to draw, which the renderer can only report as a blank
    // tile, so it is refused here where the author is still looking.
    if (nodes.length === 0 && !deriving) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["nodes"],
        message: "A graph needs at least one node, or a `derive.idField` to read them from.",
      });
    }
    // `derive` and a literal `edges` list are both ways of saying what connects
    // to what. Honouring both would leave the compiler choosing, and the two
    // answers could disagree.
    if (deriving && nodes.length > 0) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["derive"],
        message:
          "`derive` draws the nodes from the data, so it cannot be combined with a literal `nodes` list.",
      });
    }

    // Referential integrity, checked here rather than left to the renderer.
    //
    // React Flow does not reject an edge naming a node that is not in the
    // graph: it mounts the edge and renders nothing for it, so the graph comes
    // up with a silently missing connection. Catching it at the boundary turns
    // that into an authoring error with a path to the offending edge.
    //
    // Skipped when deriving: the node set is not known until the rows arrive, so
    // there is nothing to check the endpoints against here. The compiler drops a
    // dangling endpoint instead, and reports it in `diagnostics`.
    // A derived graph needs the id column, and nothing else: naming no edge
    // fields is a legitimate "one node per row, no topology" graph.
    if (deriving && spec.derive?.idField === undefined) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["derive"],
        message: "A derived graph needs a `derive.idField` naming the column holding the node id.",
      });
    }
    if (deriving && (spec.derive?.sourceField === undefined) !== (spec.derive?.targetField === undefined)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["derive"],
        message: "`derive.sourceField` and `derive.targetField` come as a pair; an edge needs both ends.",
      });
    }

    const ids = new Set<string>();
    nodes.forEach((node, i) => {
      if (ids.has(node.id)) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["nodes", i, "id"],
          message: `Duplicate node id: ${node.id}`,
        });
      }
      ids.add(node.id);
    });

    // With a derived node set the ids are unknown until the rows arrive, so an
    // endpoint can only be checked against them at render time.
    if (!deriving) {
      spec.edges.forEach((edge, i) => {
        if (!ids.has(edge.source)) {
          ctx.addIssue({
            code: z.ZodIssueCode.custom,
            path: ["edges", i, "source"],
            message: `Edge source references unknown node: ${edge.source}`,
          });
        }
        if (!ids.has(edge.target)) {
          ctx.addIssue({
            code: z.ZodIssueCode.custom,
            path: ["edges", i, "target"],
            message: `Edge target references unknown node: ${edge.target}`,
          });
        }
      });
    }
  });
export type GraphSpec = z.infer<typeof GraphSpecSchema>;

const WidgetBaseSchema = z.object({
  id: z.string().min(1),
  type: z.enum(["chart", "kpi", "table", "text", "filter", "graph"]),
  data: DataSourceSchema.optional(),
  chartSpec: ChartSpecSchema.optional(),
  chart: ChartSchema.optional(),
  /**
   * The node/edge description for a `graph` widget.
   *
   * `graph` is a SIBLING of `chart`, not a new `ChartSchema.kind`, and that is
   * the whole design decision. Every existing render path -- native kinds and
   * the chart-specs assembler alike -- terminates in an ECharts option object, and a
   * function returning an options blob cannot express a React Flow graph, whose
   * input is a `nodes`/`edges` pair it mounts through its own component tree
   * with its own drag, zoom and pan state. So the graph widget gets its own
   * terminal type, `flows` renders it, and
   * `charts` declines it instead of guessing a chart kind for it.
   */
  graph: GraphSpecSchema.optional(),
  interactions: z.array(InteractionSchema).optional(),
});

/**
 * Documents saved before the neutral rename carry `data.source: "superset"` and a
 * `flint` key. Both are still accepted on input and upgraded in place, so stored
 * dashboards keep loading; output always uses `"bi"` and `chartSpec`.
 */
function upgradeLegacyWidget(input: unknown): unknown {
  if (typeof input !== "object" || input === null || Array.isArray(input)) return input;
  const widget: Record<string, unknown> = { ...(input as Record<string, unknown>) };
  if ("flint" in widget) {
    if (!("chartSpec" in widget)) widget.chartSpec = widget.flint;
    delete widget.flint;
  }
  const data = widget.data;
  if (typeof data === "object" && data !== null && (data as { source?: unknown }).source === "superset") {
    widget.data = { ...(data as Record<string, unknown>), source: "bi" };
  }
  return widget;
}

/**
 * The terminal-type rules.
 *
 * Each rule is scoped to exactly one `type`. The chart rule predates `graph` and
 * is unchanged: `type === "chart"` must carry exactly one of `chart-specs` or `chart`.
 * The graph rules are its mirror image. Both directions matter, because the
 * failure they prevent is a widget that reaches two renderers that each believe
 * they own it -- one of which would then throw `Unknown chart kind` at the
 * author instead of drawing anything.
 */
export const WidgetSchema = z.preprocess(upgradeLegacyWidget, WidgetBaseSchema.refine(
  (data) => {
    if (data.type === "chart") {
      const hasChartSpec = data.chartSpec !== undefined;
      const hasChart = data.chart !== undefined;
      return (hasChartSpec && !hasChart) || (!hasChartSpec && hasChart);
    }
    return true;
  },
  {
    message: "Chart widgets must have exactly one of 'chartSpec' or 'chart' defined.",
  },
)
  .refine((data) => data.type !== "graph" || data.graph !== undefined, {
    message: "Graph widgets must define 'graph'.",
  })
  .refine(
    (data) => data.type !== "graph" || (data.chartSpec === undefined && data.chart === undefined),
    {
      message:
        "Graph widgets are rendered by flows, not charts; they must not declare 'chartSpec' or 'chart'.",
    },
  ));

export type Widget = z.infer<typeof WidgetSchema>;

export const LayoutItemSchema = z.object({
  id: z.string().min(1),
  x: z.number().int().min(0),
  y: z.number().int().min(0),
  w: z.number().int().min(1).max(12),
  h: z.number().int().min(1),
});
export type LayoutItem = z.infer<typeof LayoutItemSchema>;

export const ParamSchema = z.object({
  name: z.string(),
  type: z.enum(["string", "number", "date", "daterange", "select"]),
  default: z.unknown().optional(),
  datasetId: z.number().optional(),
  column: z.string().optional(),
});
export type Param = z.infer<typeof ParamSchema>;

export const DashboardSpecSchema = z.object({
  id: z.string().min(1),
  version: z.number().int().min(0),
  title: z.string().min(1).max(255),
  params: z.array(ParamSchema),
  layout: z.array(LayoutItemSchema),
  widgets: z.record(z.string(), WidgetSchema),
  /**
   * The default visual theme for every widget on this dashboard.
   *
   * PRECEDENCE: a per-widget `chartSpec.theme_spec` (see `ChartSpecSchema` above)
   * WINS over this field. That one is a per-widget override authored alongside
   * the chart spec, so it is the more specific statement; this field is the
   * house the dashboard was built in, and applies to every widget that does not
   * override it. Resolve the effective pair through
   * `ctx.chartSpecs.resolveWidgetTheme(widget, dashboardSpec.theme)` rather than
   * reading either field directly.
   *
   * Omitting it means "no theme", which is a real renderable state: chart-specs' own
   * defaults apply.
   */
  theme: ThemeSelectionSchema.optional(),
});
export type DashboardSpec = z.infer<typeof DashboardSpecSchema>;
