import { z } from "zod";

/**
 * Dashboard-level theme authoring.
 *
 * This file mirrors the SUBSET of `flint-chart@0.5.1`'s `ThemeSpec` that we
 * actually expose for authoring, and deliberately no more. Two rules govern it:
 *
 *  1. The part a customizer edits by hand (`ink`, and `type`'s colour field) is
 *     validated strictly, because a bad colour is user-visible the moment the
 *     dashboard renders.
 *  2. Everything else, and every key we do not know about, passes through
 *     untouched. A spec authored against a newer flint must survive a
 *     parse/serialise round trip through this repo without being silently
 *     stripped — otherwise storing a dashboard quietly destroys fields.
 *
 * Rule 2 is why the mirror is partial. Flint's `ThemeSpec` is level 1: it never
 * names a chart type, a channel, a mark type or a backend property, so the
 * fields we skip here (`structure`, `marks`, `legend`, `geometry`, ...) are the
 * compiler's business and a dashboard author has no business hand-writing them.
 *
 * NOTE ON ZOD VERSION: this repo resolves zod `3.25.76`, so the passthrough API
 * is the v3 `.passthrough()`. zod v4 renamed this to `z.looseObject()` and
 * dropped `.passthrough()`; if the major ever moves, this is the one call site
 * family that must change.
 */

/** A colour as written in a spec: any non-empty string. */
export const ThemeColorSchema = z.string().min(1, "a colour must be a non-empty string");
export type ThemeColor = z.infer<typeof ThemeColorSchema>;

/**
 * An interpolation ramp. `stops` is control points, not an indexed set: its
 * length is resolution, so the series-overflow rule does not apply to it.
 */
export const ThemeRampSchema = z
  .object({
    stops: z.array(ThemeColorSchema),
    neutral: ThemeColorSchema.optional(),
    space: z.enum(["rgb", "lab", "hcl"]).optional(),
    endpointsAgainstSurface: z.boolean().optional(),
    consumption: z.enum(["interpolate", "quantize", "sampleCategorical"]).optional(),
    quantizeCount: z.number().optional(),
  })
  .passthrough();
export type ThemeRamp = z.infer<typeof ThemeRampSchema>;

/**
 * `ThemeInk` — the block a customizer actually edits. Mirrors flint's
 * `ThemeInk` field for field, with every colour held to `ThemeColorSchema`.
 */
export const ThemeInkSchema = z
  .object({
    surface: z
      .object({
        source: z.enum(["host", "house"]).optional(),
        canvas: ThemeColorSchema.optional(),
        plot: ThemeColorSchema.optional(),
        panel: ThemeColorSchema.optional(),
      })
      .passthrough()
      .optional(),
    text: z
      .object({
        primary: ThemeColorSchema.optional(),
        secondary: ThemeColorSchema.optional(),
        muted: ThemeColorSchema.optional(),
        inverse: ThemeColorSchema.optional(),
      })
      .passthrough()
      .optional(),
    /** The ink the presence ordinal (`omit`..`emphasised`) scales against. */
    structure: z
      .object({
        axis: ThemeColorSchema.optional(),
        grid: ThemeColorSchema.optional(),
        frame: ThemeColorSchema.optional(),
        rule: ThemeColorSchema.optional(),
        zero: ThemeColorSchema.optional(),
        connector: ThemeColorSchema.optional(),
      })
      .passthrough()
      .optional(),
    series: z
      .object({
        single: ThemeColorSchema.optional(),
        categorical: z.array(ThemeColorSchema).optional(),
        categoricalExtended: z.array(ThemeColorSchema).optional(),
        overflow: ThemeColorSchema.optional(),
        sequential: ThemeRampSchema.optional(),
        diverging: ThemeRampSchema.optional(),
        status: z
          .object({
            positive: ThemeColorSchema.optional(),
            negative: ThemeColorSchema.optional(),
            neutral: ThemeColorSchema.optional(),
          })
          .passthrough()
          .optional(),
        selection: z
          .object({
            partToWhole: z.enum(["categorical", "sequentialRamp"]).optional(),
            signed: z.enum(["categorical", "status", "diverging", "sequential"]).optional(),
            redundantWithFacet: z.enum(["single", "categorical"]).optional(),
            statusUse: z.enum(["anySigned", "thresholdOnly", "never"]).optional(),
          })
          .passthrough()
          .optional(),
      })
      .passthrough()
      .optional(),
    accent: ThemeColorSchema.optional(),
  })
  .passthrough();
export type ThemeInk = z.infer<typeof ThemeInkSchema>;

const TypeRoleSchema = z
  .object({
    family: z.string().optional(),
    size: z.union([z.string(), z.number()]).optional(),
    weight: z.enum(["regular", "medium", "semibold", "bold"]).optional(),
    style: z.enum(["normal", "italic"]).optional(),
    case: z.enum(["asIs", "upper", "lower", "title"]).optional(),
    color: ThemeColorSchema.optional(),
  })
  .passthrough();

/**
 * The authoring subset of `ThemeSpec`.
 *
 * `ink` and `type` are mirrored because they are the two blocks a customizer
 * edits by hand. The remaining policy blocks are carried as opaque records:
 * they are the compiler's to interpret, and mirroring them here would make this
 * repo the place that breaks when flint grows one.
 */
export const ThemeSpecSchema = z
  .object({
    /** Start from a flint-shipped house, then override only what is stated. */
    extends: z.string().min(1).optional(),
    id: z.string().min(1).optional(),
    label: z.string().optional(),
    ink: ThemeInkSchema.optional(),
    type: z
      .object({
        minSize: z.number().optional(),
        headline: TypeRoleSchema.optional(),
        deck: TypeRoleSchema.optional(),
        axisLabel: TypeRoleSchema.optional(),
        axisTitle: TypeRoleSchema.optional(),
        valueLabel: TypeRoleSchema.optional(),
        keyLabel: TypeRoleSchema.optional(),
        annotation: TypeRoleSchema.optional(),
        footnote: TypeRoleSchema.optional(),
        display: TypeRoleSchema.optional(),
      })
      .passthrough()
      .optional(),
    structure: z.record(z.string(), z.unknown()).optional(),
    marks: z.record(z.string(), z.unknown()).optional(),
    labels: z.record(z.string(), z.unknown()).optional(),
    legend: z.record(z.string(), z.unknown()).optional(),
    dataLabels: z.record(z.string(), z.unknown()).optional(),
    annotation: z.record(z.string(), z.unknown()).optional(),
    furniture: z.array(z.record(z.string(), z.unknown())).optional(),
    facets: z.record(z.string(), z.unknown()).optional(),
    layout: z.record(z.string(), z.unknown()).optional(),
    geometry: z.record(z.string(), z.unknown()).optional(),
    chartDefaults: z.record(z.string(), z.record(z.string(), z.unknown())).optional(),
    compileDefaults: z.record(z.string(), z.unknown()).optional(),
    interaction: z
      .object({
        tooltipFormat: z.string().optional(),
      })
      .passthrough()
      .optional(),
    variants: z.array(z.record(z.string(), z.unknown())).optional(),
  })
  .passthrough();
export type ThemeSpec = z.infer<typeof ThemeSpecSchema>;

/**
 * A dashboard's theme choice: a shipped house by id, and/or a custom spec that
 * may itself `extends` a house.
 *
 * Both fields optional. Neither present means "no theme" — flint's own defaults
 * apply, which is a real, renderable state and not an error.
 *
 * When BOTH are given, `custom` wins and `preset` becomes its base: layering
 * happens in this repo by writing `preset` into `custom.extends` before handing
 * the spec to flint, so the merge is flint's own and keeps its semantics —
 * nested policy objects MERGE, arrays and scalars REPLACE.
 */
export const ThemeSelectionSchema = z.object({
  preset: z.string().min(1, "a theme preset id must be a non-empty string").optional(),
  custom: ThemeSpecSchema.optional(),
});
export type ThemeSelection = z.infer<typeof ThemeSelectionSchema>;

/**
 * Alias for `ThemeSelection`, named for its use: this is the dashboard-level
 * theme field, as opposed to a per-widget `flint.theme_spec`.
 */
export type DashboardTheme = ThemeSelection;

/**
 * The catalogue entry `FlintService.listThemes()` returns, mirrored here so the
 * UI can be typed without depending on `chart-specs`.
 */
export const ThemeCatalogueEntrySchema = z.object({
  id: z.string(),
  label: z.string(),
  description: z.string(),
  /** A complete 16px SVG document, ready to drop into an `<img>` or inline. */
  icon: z.string(),
});
export type ThemeCatalogueEntry = z.infer<typeof ThemeCatalogueEntrySchema>;

/**
 * Mirrors `flint-chart`'s `ThemeReport` — a downgrade or approximation.
 *
 * Silent fallbacks are indistinguishable from bugs, so every one of these is
 * surfaced to the caller rather than swallowed by the service.
 */
export const ThemeReportSchema = z.object({
  stage: z.enum(["ground", "realize"]),
  /** Dotted ThemeSpec path this concerns, e.g. `legend.placement`. */
  path: z.string(),
  message: z.string(),
});
export type ThemeReport = z.infer<typeof ThemeReportSchema>;
