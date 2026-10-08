/**
 * Flint `DesignDecisions` -> the ink a React Flow graph is painted with.
 *
 * The seam is the *grounded* theme, not the authored one, for the reason
 * `charts/src/theme-decisions.ts` states at length: `ThemeSpec` is
 * level 1 and binds no colour to any role, while `groundTheme` produces level 2,
 * with flint's ink borrowing chain, presence ordinals and accessibility rules
 * already applied. Reading a spec here would mean re-implementing all of that
 * and getting it subtly wrong. So this consumes decisions and never a spec.
 *
 * A graph is not a chart, so the mapping is smaller and there is no series to
 * recolour. What it needs is a surface, two inks of text, one rule ink for the
 * node borders and the edges, and -- the one thing that is genuinely a chart
 * decision carried over -- the categorical palette, which paints the nodes
 * exactly as it would paint the series of a bar chart of the same count.
 *
 * Every read is defensive and every field falls back independently, because a
 * theme that partially resolves is a real state: flint grounds what it can and
 * reports the rest. One absent role must not blank the whole tile.
 */

/** The colours the graph tile paints with. */
export interface FlowTheme {
  nodeBackground: string;
  nodeBorder: string;
  nodeText: string;
  edgeStroke: string;
  edgeText: string;
  /** What a selected or focused node takes. */
  accent: string;
  /** Cycled over node index for fills. */
  palette: string[];
}

/**
 * The unthemed tile.
 *
 * Every value is a CSS custom property rather than a literal, because the
 * dashboard's own tokens are what an unthemed tile should wear -- the same
 * reason `WidgetCard` reads `--bg-card` rather than hardcoding greys. This is a
 * real renderable state, not a placeholder: `resolveWidgetTheme` reports
 * `source: 'none'` when neither the dashboard nor the widget named a house, and
 * that answer means "render it as it was before themes existed".
 */
export const DEFAULT_FLOW_THEME: FlowTheme = {
  nodeBackground: "var(--bg-card)",
  nodeBorder: "var(--line)",
  nodeText: "var(--ink)",
  edgeStroke: "var(--line-subtle)",
  edgeText: "var(--ink-body)",
  accent: "var(--primary)",
  palette: [],
};

type Dict = Record<string, unknown>;

function isPlainObject(value: unknown): value is Dict {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** A non-empty string, or undefined. `transparent` counts: it is a real choice. */
function asInk(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

/** One hop through a group of decisions, for the two that are two deep. */
function at(value: unknown, key: string): unknown {
  return isPlainObject(value) ? value[key] : undefined;
}

/**
 * Flatten a grounded theme into the fields this module maps.
 *
 * Never throws and never invents: a role the decisions did not bind keeps
 * `DEFAULT_FLOW_THEME`'s value for that role and nothing else moves.
 */
export function flowThemeFromDecisions(decisions: unknown): FlowTheme {
  if (!isPlainObject(decisions)) return { ...DEFAULT_FLOW_THEME };

  const surface = at(decisions.surface, "panel") ?? at(decisions.surface, "canvas");
  const text = decisions.text;
  const series = at(decisions.series, "categorical");
  const palette = Array.isArray(series)
    ? series.filter((ink): ink is string => asInk(ink) !== undefined)
    : [];
  // The rule ink is borrowed from either place flint drew a rule, in that order:
  // the frame is the box around the whole thing, the baseline is the line marks
  // stand on, and for a graph the frame is the closer match.
  const frame = asInk(at(decisions.frame, "color"));
  const baseline = asInk(at(decisions.baseline, "color"));

  return {
    nodeBackground: asInk(surface) ?? DEFAULT_FLOW_THEME.nodeBackground,
    nodeBorder: frame ?? baseline ?? DEFAULT_FLOW_THEME.nodeBorder,
    nodeText: asInk(at(text, "primary")) ?? DEFAULT_FLOW_THEME.nodeText,
    edgeStroke: baseline ?? frame ?? DEFAULT_FLOW_THEME.edgeStroke,
    edgeText: asInk(at(text, "secondary")) ?? DEFAULT_FLOW_THEME.edgeText,
    accent: asInk(at(series, "single")) ?? palette[0] ?? DEFAULT_FLOW_THEME.accent,
    palette,
  };
}
