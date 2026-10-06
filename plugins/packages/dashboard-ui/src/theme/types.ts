/**
 * The subset of flint-chart's theme API this UI reads and writes.
 *
 * Hand-mirrored from `flint-chart@0.5.1`'s installed typings rather than
 * imported from it. `flint-chart` is not a dependency of `dashboard-ui` (the
 * server owns theme resolution), and pulling a chart compiler into the browser
 * bundle just to name a few interfaces would be the wrong trade. These are
 * structural subsets, so a spec sent here round-trips through the server
 * unchanged.
 *
 * Field meaning, per flint's own typings:
 *  - `resolveThemeSpec` takes a preset name, a ThemeSpec, or a ThemeSpec that
 *    `extends` a preset plus overrides.
 *  - Nested policy objects MERGE; arrays and scalars REPLACE.
 *  - An unknown preset name is an ERROR, never a silent fallback.
 */

export interface ThemeInk {
  surface?: {
    source?: "host" | "house";
    canvas?: string;
    plot?: string;
    panel?: string;
  };
  text?: {
    primary?: string;
    secondary?: string;
    muted?: string;
    inverse?: string;
  };
  structure?: {
    axis?: string;
    grid?: string;
    frame?: string;
    rule?: string;
    zero?: string;
    connector?: string;
  };
  series?: {
    single?: string;
    categorical?: string[];
    categoricalExtended?: string[];
    overflow?: string;
    status?: {
      positive?: string;
      negative?: string;
      neutral?: string;
    };
  };
  accent?: string;
}

export interface ThemeSpec {
  extends?: string;
  id?: string;
  label?: string;
  ink?: ThemeInk;
  [key: string]: unknown;
}

/** What a caller may store: a preset name, or a spec object. */
export type ThemeSelection = string | ThemeSpec;

/** A downgrade or approximation flint chose to make rather than swallow. */
export interface ThemeReportEntry {
  stage: string;
  path: string;
  message: string;
}

/** Row shape of GET /api/themes. */
export interface ThemePresetSummary {
  id: string;
  label: string;
  description: string;
  /** A 16px SVG *document*, documented as safe to drop into an <img>. */
  icon: string;
}

/** Body shape of GET /api/themes/:id. */
export interface ThemeResolution {
  valid: boolean;
  spec?: ThemeSpec;
  report: ThemeReportEntry[];
}

/**
 * "No house" — flint's own defaults. Persisted as a removed field rather than
 * a magic string, so clearing a theme is a removal and not a value that
 * happens to mean absence.
 */
export const NO_THEME = null;
