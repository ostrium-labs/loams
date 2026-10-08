import type { ThemeInk, ThemeSelection, ThemeSpec } from "./types";

/**
 * Ink helpers: dotted-path get/set over a spec, flint's merge semantics, and
 * the shell CSS custom properties the ink is projected onto.
 *
 * The projection is deliberately one-way and shell-only. flint's `theme_spec`
 * is applied to charts on the server (the ECharts assembler maps ThemeInk to
 * ECharts options); this side paints header, filter bar, inspector and widget
 * cards. Touching ECharts options here would duplicate the sibling's mapping
 * and the two would fight.
 */

/* ------------------------------------------------------------------ colours */

const HEX_RE = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i;

/** True when a value is a hex colour this panel can hand to <input type="color">. */
export function isHexColor(value: string): boolean {
  return HEX_RE.test(value.trim());
}

/**
 * Normalise to `#rrggbb` — the only form <input type="color"> accepts.
 * Expands `#abc`. Returns null for anything else so callers can refuse to
 * apply it rather than paint a surprise.
 */
export function toHexColor(value: string): string | null {
  const raw = value.trim();
  if (!HEX_RE.test(raw)) return null;
  const body = raw.startsWith("#") ? raw.slice(1) : raw;
  const full =
    body.length === 3
      ? body
          .split("")
          .map((c) => c + c)
          .join("")
      : body;
  return `#${full.toLowerCase()}`;
}

/** Contrast-safe text colour for a swatch, by relative luminance. */
export function readableInkOn(hex: string): string {
  const normalized = toHexColor(hex);
  if (!normalized) return "#000000";
  const r = parseInt(normalized.slice(1, 3), 16) / 255;
  const g = parseInt(normalized.slice(3, 5), 16) / 255;
  const b = parseInt(normalized.slice(5, 7), 16) / 255;
  const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
  const luminance = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  return luminance > 0.42 ? "#000000" : "#ffffff";
}

/* ------------------------------------------------------------- path get/set */

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Read a dotted path out of an object. Returns undefined for any missing link. */
export function getPath(root: unknown, path: string): unknown {
  const parts = path.split(".");
  let cursor: unknown = root;
  for (const part of parts) {
    if (!isPlainObject(cursor)) return undefined;
    cursor = cursor[part];
  }
  return cursor;
}

/**
 * Immutably set a dotted path, creating intermediate objects and dropping keys
 * whose new value is `undefined`. Dropping rather than writing `undefined`
 * matters: flint MERGES nested policy objects, so an explicit
 * `ink.surface.canvas: undefined` would still read as "stated" to the merge
 * and would mask the preset's own value on some code paths. Erasing the key is
 * the honest representation of "not overridden".
 */
export function setPath<T extends object>(root: T, path: string, value: unknown): T {
  const parts = path.split(".");
  const next = clonePlain(root) as Record<string, unknown>;
  let cursor = next;
  for (let i = 0; i < parts.length - 1; i += 1) {
    const part = parts[i];
    const existing = cursor[part];
    if (!isPlainObject(existing)) cursor[part] = {};
    cursor = cursor[part] as Record<string, unknown>;
  }
  const leaf = parts[parts.length - 1];
  if (value === undefined) delete cursor[leaf];
  else cursor[leaf] = value;
  return next as T;
}

/** Deep clone through plain objects and arrays only; values are immutable. */
function clonePlain<T>(value: T): T {
  if (Array.isArray(value)) return value.map((v) => clonePlain(v)) as unknown as T;
  if (isPlainObject(value)) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) out[k] = clonePlain(v);
    return out as unknown as T;
  }
  return value;
}

/* ------------------------------------------------------------------- merging */

/**
 * flint's inheritance rule, applied locally so the customizer can preview a
 * spec that `extends` a preset without a round-trip: nested policy objects
 * MERGE, arrays and scalars REPLACE.
 *
 * This mirrors what `resolveThemeSpec` does; it is not a reimplementation of
 * theme grounding. The server still owns the authoritative resolution and its
 * `report`.
 */
export function mergeOverBase(
  base: ThemeInk | undefined,
  overrides: ThemeInk | undefined,
): ThemeInk | undefined {
  if (!base) return overrides ? clonePlain(overrides) : undefined;
  if (!overrides) return clonePlain(base);
  return mergeInk(base, overrides);
}

function mergeInk(base: ThemeInk, overrides: ThemeInk): ThemeInk {
  const out = clonePlain(base);
  for (const [key, value] of Object.entries(overrides)) {
    if (value === undefined) continue;
    const existing = (out as Record<string, unknown>)[key];
    // Nested policy objects merge; everything else (arrays, scalars) replaces.
    if (isPlainObject(value) && isPlainObject(existing)) {
      (out as Record<string, unknown>)[key] = mergeInk(existing as ThemeInk, value as ThemeInk);
    } else {
      (out as Record<string, unknown>)[key] = clonePlain(value);
    }
  }
  return out;
}

/* ------------------------------------------------------- ink -> CSS variables */

/**
 * CSS custom properties the shell derives every themed surface from. Declared
 * with Power BI Light defaults in `styles.css`, so an unthemed dashboard looks
 * exactly as it did before theming existed, and a themed one repaints from a
 * single style write on the root element.
 */
export const THEME_CSS_VARS = {
  canvas: "--th-canvas",
  plot: "--th-plot",
  panel: "--th-panel",
  textPrimary: "--th-text-primary",
  textSecondary: "--th-text-secondary",
  textMuted: "--th-text-muted",
  axis: "--th-axis",
  grid: "--th-grid",
  rule: "--th-rule",
  seriesSingle: "--th-series-single",
  series1: "--th-series-1",
  series2: "--th-series-2",
  series3: "--th-series-3",
  series4: "--th-series-4",
  series5: "--th-series-5",
  series6: "--th-series-6",
  series7: "--th-series-7",
  series8: "--th-series-8",
} as const;

export type ThemeCssVarName = (typeof THEME_CSS_VARS)[keyof typeof THEME_CSS_VARS];

/** How many categorical slots the shell reserves for accent use. */
const SERIES_SLOTS = 8;

/**
 * Project an ink onto CSS custom properties. Only defined inks emit a value;
 * an absent field leaves the stylesheet default in place, which is what makes
 * partial customisation work (override one colour, inherit the rest).
 *
 * `series.categorical` is projected onto ordered slots because flint treats
 * array order as meaningful — slot 1 is the first series, and so on.
 */
export function inkToCssVars(ink: ThemeInk | undefined): Array<[ThemeCssVarName, string]> {
  if (!ink) return [];
  const out: Array<[ThemeCssVarName, string]> = [];
  const push = (name: ThemeCssVarName, value: string | undefined) => {
    if (value) out.push([name, value]);
  };

  push(THEME_CSS_VARS.canvas, ink.surface?.canvas);
  push(THEME_CSS_VARS.plot, ink.surface?.plot);
  push(THEME_CSS_VARS.panel, ink.surface?.panel);
  push(THEME_CSS_VARS.textPrimary, ink.text?.primary);
  push(THEME_CSS_VARS.textSecondary, ink.text?.secondary);
  push(THEME_CSS_VARS.textMuted, ink.text?.muted);
  push(THEME_CSS_VARS.axis, ink.structure?.axis);
  push(THEME_CSS_VARS.grid, ink.structure?.grid);
  push(THEME_CSS_VARS.rule, ink.structure?.rule);
  push(THEME_CSS_VARS.seriesSingle, ink.series?.single);

  const slots = [
    THEME_CSS_VARS.series1,
    THEME_CSS_VARS.series2,
    THEME_CSS_VARS.series3,
    THEME_CSS_VARS.series4,
    THEME_CSS_VARS.series5,
    THEME_CSS_VARS.series6,
    THEME_CSS_VARS.series7,
    THEME_CSS_VARS.series8,
  ] as const;
  const categorical = ink.series?.categorical;
  if (categorical) {
    for (let i = 0; i < Math.min(categorical.length, SERIES_SLOTS); i += 1) {
      push(slots[i], categorical[i]);
    }
  }
  return out;
}

/**
 * A swatch strip mirroring the categorical palette in order — the shell's
 * stand-in for a chart's series colours, and the thing that makes reordering
 * in the customizer legible without saving.
 */
export function categoricalSwatches(ink: ThemeInk | undefined): string[] {
  return ink?.series?.categorical?.filter((c) => typeof c === "string" && c.length > 0) ?? [];
}

/** A stable, human-readable name for a selection, for headings and save copy. */
export function describeSelection(selection: ThemeSelection | null | undefined): string {
  if (selection === null || selection === undefined || selection === "") return "Flint defaults";
  if (typeof selection === "string") return selection;
  return selection.label || selection.extends || "Custom";
}
