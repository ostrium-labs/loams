/**
 * Flint `DesignDecisions` -> ECharts option keys.
 *
 * flint-chart's visual theming is realized by its Vega-Lite assembler only; its
 * ECharts assembler accepts a `theme_spec` and discards it. This module is the
 * missing half of that contract for the ECharts path.
 *
 * The seam is deliberately the *grounded* theme, not the authored one. A
 * `ThemeSpec` is level 1: it states ink and policy without binding any of it to
 * a chart. `groundTheme` turns it into `DesignDecisions` - level 2, still
 * backend-neutral, but with every role bound to this chart. Reading the spec
 * directly would mean re-implementing flint's presence ordinals, its ink
 * borrowing chain and its accessibility rules, and getting them subtly wrong.
 * So this module consumes decisions and never a spec.
 *
 * Rules this module holds to:
 *
 * 1. Pure and copy-on-write. The input option object is never mutated; new
 *    objects are constructed along each written path, and untouched subtrees
 *    keep their identity so "no theme says anything here" stays a true no-op.
 * 2. Never invents ink. Every write is gated on a decision actually naming a
 *    colour. Silence yields an option tree deeply equal to the input.
 * 3. Colours and the presence flags that belong to them, and nothing else. This
 *    is an ink bridge: no font sizes, no rotation, no padding. Structural chart
 *    chrome belongs to the compiler and to the card that renders it.
 * 4. No runtime dependency on `echarts`; only structural option types are used.
 *
 * Precedence: this is an overlay applied *after* compilation, because a theme
 * that silently lost to a compiler default is exactly the failure mode this
 * module exists to remove.
 */
import type { DesignDecisions } from "flint-chart/core";

/** A single ECharts option object. Deliberately structural, not echarts' own types. */
export type EChartsOptionLike = Record<string, unknown>;

type Dict = Record<string, unknown>;

/** The screen channel of one axis. flint's `DesignDecisions.axes` is keyed by these. */
export type AxisChannel = "x" | "y";

/**
 * One axis, from both sides.
 *
 * flint keys its bound axes by screen channel (`x`, `y`); ECharts keys its
 * option by axis name (`xAxis`, `yAxis`). Nothing else in the two trees shares a
 * name for an axis, so the pairing is stated once here rather than at each site.
 */
const AXES = [
  { channel: "x", optionKey: "xAxis" },
  { channel: "y", optionKey: "yAxis" },
] as const;

/**
 * The chart facts grounding is allowed to consult.
 *
 * Structurally a subset of `@loams-plugins/plugin-flint`'s `GroundingFacts`: every field
 * optional, so this type is assignable to it without pulling plugin-flint into a
 * module that has no other reason to depend on it.
 *
 * What is NOT here is as deliberate as what is. `markChannel` is absent because
 * this path is not told which cognitive channel a chart's marks serve, and
 * naming one (`'length'` for a bar, say) would make grounding resolve policy
 * against a channel the chart may not have. `canvasSize` is absent because the
 * option is compiled without a layout. `hostSurface` is absent because the card
 * that eventually paints this option may sit on a different surface than the
 * one compiled here. `stacked` is absent because ECharts states a stack by NAME
 * and offers no way to tell a normalized stack from a plain one. Omitting a fact
 * is honest; guessing one is how a theme starts lying.
 */
export interface ThemeGroundingFacts {
  markTypes?: string[];
  channelSemantics?: Record<string, unknown>;
  table?: unknown[];
  titled?: boolean;
  partToWhole?: boolean;
}

// ─── Chart facts ────────────────────────────────────────────────────────────

/**
 * ECharts series type -> flint `GeometryKind`.
 *
 * flint's mark vocabulary is `'line' | 'point' | 'area' | 'band' | 'arc' |
 * 'cell'`. A bar is `band` (a bar in a row) rather than `cell` (a tile in a
 * grid); a pie is `arc`. Only these five series types are translated: one this
 * table does not name contributes no mark family rather than a wrong one.
 */
const MARK_FAMILY_BY_SERIES_TYPE: Dict = {
  bar: "band",
  line: "line",
  scatter: "point",
  pie: "arc",
  heatmap: "cell",
};

/** Series whose single mark is a slice of a whole: one ink per data point. */
const PART_TO_WHOLE_SERIES_TYPES = new Set(["pie", "sunburst", "treemap"]);

/**
 * Series types whose pieces are held apart by a separator rule.
 *
 * The rule is `marks.slice` for a part-to-whole mark and `marks.tile` for a
 * cell. ECharts draws both with `itemStyle.borderColor`/`borderWidth`, so the
 * two decision blocks land on the same pair of keys.
 */
const SEPARATOR_RULE_BY_SERIES_TYPE: Dict = {
  pie: "slice",
  sunburst: "slice",
  treemap: "slice",
  heatmap: "tile",
};

function isPlainObject(value: unknown): value is Dict {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function readGroup(value: unknown): Dict | undefined {
  return isPlainObject(value) ? value : undefined;
}

/** A string value out of one of the tables above, or nothing. */
function lookup(table: Dict, key: string): string | undefined {
  return asString(table[key]);
}

function seriesList(options: EChartsOptionLike): Dict[] {
  if (!Array.isArray(options.series)) return [];
  return options.series.filter(isPlainObject);
}

function seriesTypes(options: EChartsOptionLike): string[] {
  const types: string[] = [];
  for (const entry of seriesList(options)) {
    const type = asString(entry.type);
    if (type !== undefined) types.push(type);
  }
  return types;
}

/**
 * Per-channel semantics, derived from what the option itself states.
 *
 * A `'value'` axis IS a quantitative channel in ECharts' own vocabulary, so it
 * is stated as one. A `'category'` axis is given no type at all: ECharts does not
 * distinguish nominal from ordinal from temporal, and inventing one would let
 * grounding resolve title and truncation policy against a semantic claim nobody
 * made. An axis the option does not carry is absent from the record, which is
 * what makes grounding bind no decision for it.
 */
function readChannelSemantics(options: EChartsOptionLike): Record<string, unknown> {
  const semantics: Record<string, unknown> = {};
  const series = seriesList(options)[0];
  const encode = readGroup(series?.encode);
  for (const { channel, optionKey } of AXES) {
    const axes = options[optionKey];
    const first = Array.isArray(axes) ? axes[0] : axes;
    const type = asString(readGroup(first)?.type);
    if (type === undefined) continue;
    const encoded = encode?.[channel];
    const field =
      typeof encoded === "string"
        ? encoded
        : Array.isArray(encoded)
          ? asString(encoded[0])
          : undefined;
    semantics[channel] = type === "value" ? { field, type: "quantitative" } : { field };
  }
  return semantics;
}

/**
 * Facts about the compiled chart, stated only where they are known.
 *
 * Called AFTER compilation, because compilation is what turns a widget into
 * something with axes, series and a title: the facts grounding wants are
 * properties of that tree, not of the widget that asked for it.
 */
export function buildThemeFacts(options: EChartsOptionLike, rows?: unknown[]): ThemeGroundingFacts {
  const markTypes = seriesTypes(options)
    .map((type) => lookup(MARK_FAMILY_BY_SERIES_TYPE, type))
    .filter((family): family is string => family !== undefined);

  const facts: ThemeGroundingFacts = { channelSemantics: readChannelSemantics(options) };
  if (markTypes.length > 0) facts.markTypes = [...new Set(markTypes)];
  if (asString(readGroup(options.title)?.text) !== undefined) facts.titled = true;
  if (seriesTypes(options).some((type) => PART_TO_WHOLE_SERIES_TYPES.has(type)))
    facts.partToWhole = true;
  if (rows !== undefined) facts.table = rows;
  return facts;
}

// ─── The grounded ink, flattened ────────────────────────────────────────────

/** A rule: a hairline the house either draws or omits, in a named ink. */
export interface GroundedRule {
  show: boolean;
  color?: string;
  width?: number;
  dash?: number[];
}

/** One axis' grounded ink. Every field is absent when grounding bound nothing. */
export interface GroundedAxis {
  domain?: GroundedRule;
  ticks?: GroundedRule;
  grid?: GroundedRule;
  label?: string;
  title?: string;
}

/**
 * The subset of `DesignDecisions` this mapping consumes.
 *
 * Flattened and pre-validated once, so every mapping site below is free of
 * defensive noise. Same reason the ink-first version of this module had a
 * reader: normalize once, at the boundary, not at every use.
 */
export interface GroundedInk {
  canvas?: string;
  panel?: string;
  textPrimary?: string;
  textSecondary?: string;
  axes: Record<AxisChannel, GroundedAxis>;
  /** The rule the marks stand on: the axis-line ink when no axis is bound. */
  baseline?: GroundedRule;
  frame?: GroundedRule;
  palette?: {
    single?: string;
    categorical: string[];
    overflow?: string;
    /** The house declined to impose its set; what is on the chart was chosen for the count. */
    exhausted?: boolean;
  };
  /** Control points for a continuous colour scale, when grounding sampled one. */
  ramp?: string[];
  legend?: { show?: boolean; label?: string };
  /** Value-label ink, and the mode that says whether it is a fixed ink at all. */
  dataLabels?: { inkMode?: string; color?: string };
  separators?: Record<string, { color?: string; gap?: number }>;
}

/** `'transparent'` is flint's "no ink", not a colour to write down. */
function readRule(value: unknown): GroundedRule | undefined {
  const rule = readGroup(value);
  if (!rule) return undefined;
  const show = rule.show !== false;
  const out: GroundedRule = { show };
  const color = show ? asString(rule.color) : undefined;
  if (color !== undefined && color !== "transparent") out.color = color;
  if (typeof rule.width === "number" && rule.width > 0) out.width = rule.width;
  if (Array.isArray(rule.dash)) {
    const dash = rule.dash.filter((step): step is number => typeof step === "number");
    if (dash.length > 0) out.dash = dash;
  }
  return out;
}

function readAxis(decisions: DesignDecisions, channel: AxisChannel): GroundedAxis {
  const axis = decisions.axes?.[channel];
  if (!axis) return {};
  const out: GroundedAxis = {};
  const domain = readRule(axis.domain);
  const ticks = readRule(axis.ticks);
  const grid = readRule(axis.grid);
  if (domain !== undefined) out.domain = domain;
  if (ticks !== undefined) out.ticks = ticks;
  if (grid !== undefined) out.grid = grid;
  const label = asString(axis.label?.color);
  const title = asString(axis.title?.color);
  if (label !== undefined) out.label = label;
  if (title !== undefined) out.title = title;
  return out;
}

function readGap(value: unknown): number | undefined {
  return typeof value === "number" && value > 0 ? value : undefined;
}

/** Flatten a grounded theme into the fields this module maps. */
export function readGroundedInk(decisions: DesignDecisions): GroundedInk {
  const series = decisions.series;
  const categorical = Array.isArray(series?.categorical)
    ? series.categorical.filter((ink): ink is string => typeof ink === "string" && ink.length > 0)
    : [];

  const single = asString(series?.single);
  const overflow = asString(series?.overflow);
  const ramp = series?.range ?? series?.ramp?.stops;
  const marks = decisions.marks;

  const ink: GroundedInk = { axes: { x: readAxis(decisions, "x"), y: readAxis(decisions, "y") } };

  const canvas = asString(decisions.surface?.canvas);
  if (canvas !== undefined) ink.canvas = canvas;
  const panel = asString(decisions.surface?.panel);
  if (panel !== undefined) ink.panel = panel;
  const primary = asString(decisions.text?.primary);
  const secondary = asString(decisions.text?.secondary);
  if (primary !== undefined) ink.textPrimary = primary;
  if (secondary !== undefined) ink.textSecondary = secondary;

  const baseline = readRule(decisions.baseline);
  if (baseline !== undefined) ink.baseline = baseline;
  const frame = readRule(decisions.frame);
  if (frame !== undefined) ink.frame = frame;

  if (single !== undefined || categorical.length > 0) {
    const palette: NonNullable<GroundedInk["palette"]> = { categorical };
    if (single !== undefined) palette.single = single;
    if (overflow !== undefined) palette.overflow = overflow;
    if (series?.exhausted === true) palette.exhausted = true;
    ink.palette = palette;
  }

  if (Array.isArray(ramp)) {
    const stops = ramp.filter(
      (stop): stop is string => typeof stop === "string" && stop.length > 0,
    );
    if (stops.length > 0) ink.ramp = stops;
  }

  if (decisions.legend) {
    const entry: NonNullable<GroundedInk["legend"]> = {};
    if (typeof decisions.legend.show === "boolean") entry.show = decisions.legend.show;
    const legendInk = asString(decisions.legend.label?.color);
    if (legendInk !== undefined) entry.label = legendInk;
    if (Object.keys(entry).length > 0) ink.legend = entry;
  }

  if (decisions.dataLabels) {
    const entry: NonNullable<GroundedInk["dataLabels"]> = {};
    const mode = asString(decisions.dataLabels.inkMode);
    const color = asString(decisions.dataLabels.text?.color);
    if (mode !== undefined) entry.inkMode = mode;
    if (color !== undefined) entry.color = color;
    if (Object.keys(entry).length > 0) ink.dataLabels = entry;
  }

  const separators: NonNullable<GroundedInk["separators"]> = {};
  const slice = readGroup(marks?.slice);
  const tile = readGroup(marks?.tile);
  if (slice) {
    const color = asString(slice.color);
    if (color !== undefined) separators.slice = { color, gap: readGap(slice.gap) };
  }
  if (tile) {
    const color = asString(tile.color);
    if (color !== undefined) separators.tile = { color, gap: readGap(tile.gap) };
  }
  if (Object.keys(separators).length > 0) ink.separators = separators;

  return ink;
}

// ─── The mapping ────────────────────────────────────────────────────────────

/**
 * Copy-on-write path setter. Returns the *same* reference when the value already
 * sits at that path, which is what keeps the silent-theme case a true no-op.
 * Intermediate levels that are missing (or the `axisLine: true` boolean
 * shorthand) are expanded into the object form, which ECharts treats identically.
 */
function setIn(base: Dict, path: readonly string[], value: unknown): Dict {
  const [head, ...rest] = path;
  if (head === undefined) return base;
  const current = base[head];
  const next =
    rest.length === 0 ? value : setIn(isPlainObject(current) ? current : {}, rest, value);
  if (current === next) return base;
  return { ...base, [head]: next };
}

/** Apply `fn` to a value ECharts accepts as either one object or an array. */
function mapOneOrMany(value: unknown, fn: (item: Dict) => Dict): unknown {
  if (Array.isArray(value)) {
    let changed = false;
    const next = value.map((item) => {
      if (!isPlainObject(item)) return item;
      const styled = fn(item);
      if (styled !== item) changed = true;
      return styled;
    });
    return changed ? next : value;
  }
  if (!isPlainObject(value)) return value;
  const styled = fn(value);
  return styled === value ? value : styled;
}

/** The row count a dataset-bound series would draw, when the option states it. */
function datasetRowCount(options: EChartsOptionLike): number | undefined {
  const dataset = options.dataset;
  const first = Array.isArray(dataset) ? dataset[0] : dataset;
  const source = readGroup(first)?.source;
  return Array.isArray(source) ? source.length : undefined;
}

/**
 * How many distinct inks the chart needs from `color`.
 *
 * Ordinary series consume one ink each. Part-to-whole families consume one per
 * slice, because ECharts cycles `color` across a pie's slices, and those slices
 * come from `data` or, on a dataset-bound series, from the rows the caller
 * fetched (or, failing that, from the dataset the option carries).
 */
export function countColorDemands(options: EChartsOptionLike, rows?: unknown[]): number {
  const series = seriesList(options);
  if (series.length === 0) return 0;
  const rowCount = rows?.length ?? datasetRowCount(options);
  let needed = 0;
  for (const entry of series) {
    if (PART_TO_WHOLE_SERIES_TYPES.has(asString(entry.type) ?? "")) {
      needed += Array.isArray(entry.data)
        ? Math.max(entry.data.length, 1)
        : Math.max(rowCount ?? 1, 1);
      continue;
    }
    needed += 1;
  }
  return needed;
}

/**
 * The palette a grounded theme implies for ECharts' `color`.
 *
 * One mark takes the house's single ink; several take its categorical set, with
 * the overflow ink appended when the chart needs more names than the house
 * named. `exhausted` means grounding decided the house set does NOT apply at
 * this cardinality, which is an instruction not to impose it, so the option
 * keeps whatever colour it already had. Undefined means "say nothing".
 */
export function resolvePalette(ink: GroundedInk, colorDemands: number): string[] | undefined {
  const palette = ink.palette;
  if (!palette || palette.exhausted) return undefined;
  if (colorDemands <= 1) {
    const one = palette.single ?? palette.categorical[0];
    return one === undefined ? undefined : [one];
  }
  if (palette.categorical.length === 0) return undefined;
  const out = [...palette.categorical];
  if (palette.overflow !== undefined && colorDemands > out.length) out.push(palette.overflow);
  return out;
}

/**
 * Write one rule onto one of ECharts' axis sub-blocks.
 *
 * A rule the house omits is written as `show: false` rather than skipped:
 * skipping the write would leave the compiler's or the card's default line in
 * place, which is the house's decision being overruled by an accident.
 */
function writeRule(target: Dict, key: string, rule: GroundedRule): Dict {
  let out = setIn(target, [key, "show"], rule.show);
  if (rule.color !== undefined) out = setIn(out, [key, "lineStyle", "color"], rule.color);
  if (rule.width !== undefined) out = setIn(out, [key, "lineStyle", "width"], rule.width);
  if (rule.dash !== undefined) out = setIn(out, [key, "lineStyle", "type"], "dashed");
  return out;
}

/** Apply the grounded ink of one axis. */
function styleAxis(axis: Dict, ink: GroundedInk, channel: AxisChannel): Dict {
  let out = axis;
  const grounded = ink.axes[channel];

  // The axis line is the axis's own rule when grounding bound one, else the rule
  // the marks stand on, else the plot frame. A house that draws none of them
  // says so with `show: false` rather than by falling silent.
  const rule = grounded.domain ?? ink.baseline ?? ink.frame;
  if (rule !== undefined) {
    out = writeRule(out, "axisLine", rule);
    out = writeRule(out, "axisTick", grounded.ticks ?? rule);
  } else if (grounded.ticks !== undefined) {
    out = writeRule(out, "axisTick", grounded.ticks);
  }

  if (grounded.grid !== undefined) out = writeRule(out, "splitLine", grounded.grid);

  const labelInk = grounded.label ?? ink.textSecondary;
  if (labelInk !== undefined) out = setIn(out, ["axisLabel", "color"], labelInk);

  const titleInk = grounded.title ?? ink.textSecondary;
  if (titleInk !== undefined) out = setIn(out, ["nameTextStyle", "color"], titleInk);

  return out;
}

/**
 * Apply the grounded ink of one series.
 *
 * Value labels are only inked when the house declares a FIXED label ink. Under
 * `matchSeries` the label wears its mark's colour, and under
 * `contrastWithMark` it wears whatever reads against that mark; neither is a
 * colour this module may write down without knowing the fill.
 */
function styleSeries(series: Dict, ink: GroundedInk): Dict {
  let out = series;

  if (
    isPlainObject(series.label) &&
    ink.dataLabels?.inkMode === "fixed" &&
    ink.dataLabels.color !== undefined
  ) {
    out = setIn(out, ["label", "color"], ink.dataLabels.color);
  }

  const separatorKey = lookup(SEPARATOR_RULE_BY_SERIES_TYPE, asString(series.type) ?? "");
  const separator = separatorKey === undefined ? undefined : ink.separators?.[separatorKey];
  if (separator?.gap !== undefined) {
    if (separator.color !== undefined)
      out = setIn(out, ["itemStyle", "borderColor"], separator.color);
    out = setIn(out, ["itemStyle", "borderWidth"], separator.gap);
  }

  return out;
}

/** Tooltip chrome: the house's own panel, its hairline edge, its label ink. */
function styleTooltip(tooltip: Dict, ink: GroundedInk): Dict {
  let out = tooltip;
  const surface = ink.panel ?? ink.canvas;
  if (surface !== undefined) out = setIn(out, ["backgroundColor"], surface);

  // A tooltip is a panel standing on the canvas, so its edge reads against the
  // canvas. Where the house has no separate panel the edge is its own text ink.
  const edge = ink.panel !== undefined && ink.panel !== ink.canvas ? ink.canvas : ink.textSecondary;
  if (edge !== undefined) {
    out = setIn(out, ["borderColor"], edge);
    out = setIn(out, ["borderWidth"], 1);
  }

  if (ink.textPrimary !== undefined) out = setIn(out, ["textStyle", "color"], ink.textPrimary);
  return out;
}

/**
 * Overlay a grounded theme's ink onto an ECharts option tree.
 *
 * Returns a new option object; the input is never mutated. When the theme names
 * no applicable ink the input reference is returned unchanged.
 *
 * MAPPED - the full table, and where each key's ink comes from:
 *
 * | ECharts option               | Grounded decision                            |
 * | ---------------------------- | -------------------------------------------- |
 * | `color`                      | `series.single`, else `series.categorical` (+ `series.overflow`) |
 * | `backgroundColor`            | `surface.canvas`                             |
 * | `*.axisLine`, `*.axisTick`   | `axes[ch].domain`, `.ticks`; else `baseline`; else `frame` |
 * | `*.splitLine`                | `axes[ch].grid`                              |
 * | `*.axisLabel.color`          | `axes[ch].label.color`, else `text.secondary` |
 * | `*.nameTextStyle.color`      | `axes[ch].title.color`, else `text.secondary` |
 * | `series[].label.color`       | `dataLabels.text.color`, when `inkMode === 'fixed'` |
 * | `series[].itemStyle.border*` | `marks.slice` (wedge) / `marks.tile` (cell)  |
 * | `tooltip.backgroundColor`    | `surface.panel`, else `surface.canvas`       |
 * | `tooltip.borderColor`        | `surface.canvas` when the panel differs, else `text.secondary` |
 * | `tooltip.textStyle.color`    | `text.primary`                               |
 * | `legend.textStyle.color`     | `legend.label.color`, else `text.secondary`  |
 * | `legend.show`                | `legend.show === false`                      |
 * | `title.textStyle.color`      | `text.primary`                               |
 * | `title.subtextStyle.color`   | `text.secondary`                             |
 * | `visualMap.inRange.color`    | `series.range`, else `series.ramp.stops`     |
 * | `visualMap.textStyle.color`  | `text.secondary`                             |
 *
 * UNMAPPED, deliberately:
 *
 * - `surface.plot` - ECharts has no plot-area fill option; `plotBackgroundColor`
 *   is a Highcharts concept, and inventing one would need a synthetic background
 *   series, which would change hit-testing.
 * - `series.status.*` and `series.mode: 'status'` - which series is "positive" is
 *   a semantic judgement no option tree carries. Applying a status ink
 *   positionally would colour series by order, which is a lie.
 * - `series.overflow` as a *tail* - ECharts cycles `color` modulo its length, so
 *   an appended tail ink would collide with the core palette instead of
 *   extending it. It is appended as one more name instead.
 * - `text.inverse` - nothing in this tree is guaranteed to sit on the house's
 *   canvas colour, so inverse text could be unreadable.
 * - `dataLabels.show` / `possible` - grounding answers a density question that
 *   needs a banded axis, which this path does not supply. Imposing the answer
 *   would erase labels the house would have drawn.
 * - `facets`, `layout`, `furniture`, `statistics`, `title.*`, `pointEmphasis`,
 *   `marks.point/outline/connector/interval/summary/reference/observations`,
 *   `axes.zeroRule`, `axes.unit`, `interaction.tooltipFormat` - no unambiguous
 *   option key, or the realizer belongs to the assembler rather than to an
 *   overlay.
 */
export function applyGroundedInk(
  options: EChartsOptionLike,
  decisions: DesignDecisions,
  rows?: unknown[],
): EChartsOptionLike {
  if (!isPlainObject(options)) return options;
  const ink = readGroundedInk(decisions);

  let out = options;

  if (ink.canvas !== undefined) out = setIn(out, ["backgroundColor"], ink.canvas);

  const palette = resolvePalette(ink, countColorDemands(out, rows));
  if (palette !== undefined) out = setIn(out, ["color"], palette);

  for (const { channel, optionKey } of AXES) {
    const axes = mapOneOrMany(out[optionKey], (axis) => styleAxis(axis, ink, channel));
    if (axes !== out[optionKey]) out = setIn(out, [optionKey], axes);
  }

  if (Array.isArray(out.series)) {
    let changed = false;
    const series = out.series.map((entry) => {
      if (!isPlainObject(entry)) return entry;
      const styled = styleSeries(entry, ink);
      if (styled !== entry) changed = true;
      return styled;
    });
    if (changed) out = setIn(out, ["series"], series);
  }

  const titles = mapOneOrMany(out.title, (title) => {
    let styled = title;
    if (ink.textPrimary !== undefined)
      styled = setIn(styled, ["textStyle", "color"], ink.textPrimary);
    if (ink.textSecondary !== undefined)
      styled = setIn(styled, ["subtextStyle", "color"], ink.textSecondary);
    return styled;
  });
  if (titles !== out.title) out = setIn(out, ["title"], titles);

  const legends = mapOneOrMany(out.legend, (legend) => {
    let styled = legend;
    const legendInk = ink.legend?.label ?? ink.textSecondary;
    if (legendInk !== undefined) styled = setIn(styled, ["textStyle", "color"], legendInk);
    if (ink.legend?.show === false) styled = setIn(styled, ["show"], false);
    return styled;
  });
  if (legends !== out.legend) out = setIn(out, ["legend"], legends);

  if (ink.ramp !== undefined && out.visualMap !== undefined) {
    const visualMaps = mapOneOrMany(out.visualMap, (visualMap) => {
      let styled = setIn(visualMap, ["inRange", "color"], ink.ramp);
      if (ink.textSecondary !== undefined)
        styled = setIn(styled, ["textStyle", "color"], ink.textSecondary);
      return styled;
    });
    if (visualMaps !== out.visualMap) out = setIn(out, ["visualMap"], visualMaps);
  }

  // A tooltip that does not exist yet is not created: the compiler decides
  // whether a chart has one, and an overlay that invented a trigger would change
  // what the reader can interrogate.
  if (isPlainObject(out.tooltip)) out = setIn(out, ["tooltip"], styleTooltip(out.tooltip, ink));

  return out;
}

/**
 * Grounded decision blocks with no ECharts option key.
 *
 * Reported at debug level, because a house that is only partly realized should
 * be visible in the logs rather than silently half-applied. Flint's own rule is
 * that a silent fallback is indistinguishable from a bug.
 */
export function collectUnmappedDecisions(decisions: DesignDecisions): string[] {
  const unmapped: string[] = [];
  const bound = AXES.filter(({ channel }) => decisions.axes?.[channel] !== undefined);
  if (bound.length === 0) unmapped.push("axes.*");
  for (const { channel } of bound) {
    const axis = decisions.axes[channel];
    if (!axis) continue;
    if (axis.zeroRule !== undefined) unmapped.push(`axes.${channel}.zeroRule`);
    if (axis.unit !== undefined) unmapped.push(`axes.${channel}.unit`);
    if (axis.tickLabels !== undefined) unmapped.push(`axes.${channel}.tickLabels`);
  }
  if (decisions.title !== undefined) unmapped.push("title.*");
  if (decisions.facets !== undefined) unmapped.push("facets.*");
  if (decisions.layout !== undefined) unmapped.push("layout.*");
  if (decisions.furniture !== undefined) unmapped.push("furniture");
  if (decisions.statistics !== undefined) unmapped.push("statistics");
  if (decisions.pointEmphasis !== undefined) unmapped.push("pointEmphasis");
  if (decisions.series?.status !== undefined) unmapped.push("series.status.*");
  if (decisions.series?.mode === "status") unmapped.push("series.mode=status");
  const marks = decisions.marks;
  if (marks) {
    const blocks = [
      "point",
      "outline",
      "connector",
      "interval",
      "summary",
      "reference",
      "observations",
    ] as const;
    for (const block of blocks) {
      if (marks[block] !== undefined) unmapped.push(`marks.${block}`);
    }
  }
  return unmapped;
}
