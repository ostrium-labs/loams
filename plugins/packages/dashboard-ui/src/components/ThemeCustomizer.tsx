import React from "react";
import { Label, Separator, cn } from "@loams-plugins/core/ui";
import { ColorField, ColorChip } from "./ColorField";
import { CategoricalPaletteEditor } from "./CategoricalPaletteEditor";
import { CONTROL_SELECT, GROUP, HELP, LABEL } from "./fields";
import { getPath, mergeOverBase, setPath } from "../theme/ink";
import type { ThemeInk, ThemePresetSummary, ThemeSpec } from "../theme/types";

interface ThemeCustomizerProps {
  /** The custom spec being edited. `extends` is preserved, never written here. */
  spec: ThemeSpec;
  /** The preset the spec inherits from, resolved server-side. */
  baseInk: ThemeInk | undefined;
  extendsId: string | null;
  presets: ThemePresetSummary[];
  /** The spec's own overrides, without the inherited base. */
  onChange: (next: ThemeSpec) => void;
}

/* ------------------------------------------------------------- fragments */

const ROOT = "flex flex-col gap-[1.1rem]";

const INHERIT_BANNER = "rounded border border-primary-border bg-primary-subtle p-[0.75rem_0.85rem]";

const INHERIT_HEAD = "mb-1 text-[0.82rem] text-primary-ink";

const INHERIT_BODY = "text-[0.74rem] leading-[1.45] text-ink-body";

/*
 * A group of related colour fields.
 *
 * A `<fieldset>`/`<legend>` rather than a div, so a screen reader announces the
 * group name when focus enters it and the fields inside are reachable as a
 * set. `m-0` because preflight resets the default UA margin.
 */
const FIELDSET =
  "m-0 flex flex-col gap-[0.8rem] rounded border border-line p-[0.85rem_0.9rem_0.95rem]";

const LEGEND = "px-[0.35rem] text-[0.72rem] font-bold tracking-[0.04em] text-ink-muted uppercase";

/** The read-only "what is actually applied" readout at the bottom. */
const EFFECTIVE = "rounded border border-line-subtle p-[0.6rem_0.8rem]";

const EFFECTIVE_SUMMARY = "cursor-pointer text-[0.78rem] font-semibold text-ink";

const EFFECTIVE_GRID = "mt-[0.55rem] flex flex-wrap gap-[0.6rem]";

/**
 * Editor for the overridable ink fields.
 *
 * Reads through the inheritance: an unset field shows the value it would
 * inherit, greyed, so the user can see that `extends` is doing work rather than
 * guessing. Only fields the user actually touched are written into the spec —
 * that keeps the override set minimal, which matters because arrays and scalars
 * REPLACE rather than merge, so an override nobody intended would quietly drop
 * a preset's colours.
 */
export const ThemeCustomizer: React.FC<ThemeCustomizerProps> = ({
  spec,
  baseInk,
  extendsId,
  presets,
  onChange,
}) => {
  const own = spec.ink;
  const effective = mergeOverBase(baseInk, own);

  /** Writes one dotted path into the spec's own ink, or clears it on reset. */
  const edit = (path: string, value: unknown) => {
    onChange(setPath(spec, `ink.${path}`, value));
  };

  const inherited = (path: string) => {
    const base = getPath(baseInk, path) as string | undefined;
    return typeof base === "string" ? base : undefined;
  };

  const effectiveColor = (path: string) => {
    const own_ = getPath(own, path) as string | undefined;
    if (typeof own_ === "string") return own_;
    return inherited(path);
  };

  const isInherited = (path: string) => typeof getPath(own, path) !== "string" && !!inherited(path);

  const categorical =
    (getPath(own, "series.categorical") as string[] | undefined) ??
    baseInk?.series?.categorical ??
    [];

  return (
    <div className={ROOT}>
      {/* ---- Inheritance hint ------------------------------------------- */}
      <div className={INHERIT_BANNER}>
        <div className={INHERIT_HEAD}>
          <strong>
            {extendsId ? `Extends “${labelFor(presets, extendsId)}”` : "Standalone theme"}
          </strong>
        </div>
        <p className={cn(INHERIT_BODY, "m-0")}>
          {extendsId ? (
            <>
              Every field you leave alone keeps the house&apos;s value, which is why the rest of the
              dashboard still looks themed. What you do set
              <em> replaces</em> that field — nested objects merge with what they overlap, but
              colours and colour arrays are taken whole.
            </>
          ) : (
            <>
              This theme extends nothing, so only the fields you set below apply. Every other colour
              falls back to the dashboard&apos;s own defaults. Pick a preset under{" "}
              <em>Base theme</em> to inherit the rest.
            </>
          )}
        </p>
      </div>

      {/* ---- Base theme --------------------------------------------------- */}
      <div className={GROUP}>
        <Label className={LABEL} htmlFor="theme-extends">
          Base theme
        </Label>
        <select
          id="theme-extends"
          className={CONTROL_SELECT}
          value={extendsId ?? ""}
          onChange={(e) => {
            const next = e.target.value;
            onChange({ ...spec, extends: next.length > 0 ? next : undefined });
          }}
        >
          <option value="">None — standalone theme</option>
          {presets.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </select>
      </div>

      <Separator />

      {/* ---- Surface ------------------------------------------------------ */}
      <fieldset className={FIELDSET}>
        <legend className={LEGEND}>Surfaces</legend>
        <ColorField
          id="theme-surface-canvas"
          label="Canvas"
          hint="Page background behind everything."
          value={effectiveColor("surface.canvas")}
          onChange={(v) => edit("surface.canvas", v)}
          onClear={
            isInherited("surface.canvas") ? () => edit("surface.canvas", undefined) : undefined
          }
        />
        <ColorField
          id="theme-surface-plot"
          label="Plot"
          hint="Area a chart is drawn on."
          value={effectiveColor("surface.plot")}
          onChange={(v) => edit("surface.plot", v)}
          onClear={isInherited("surface.plot") ? () => edit("surface.plot", undefined) : undefined}
        />
      </fieldset>

      {/* ---- Text --------------------------------------------------------- */}
      <fieldset className={FIELDSET}>
        <legend className={LEGEND}>Text</legend>
        {(
          [
            ["text.primary", "Primary"],
            ["text.secondary", "Secondary"],
            ["text.muted", "Muted"],
          ] as const
        ).map(([path, label]) => (
          <ColorField
            key={path}
            id={`theme-${path.replace(".", "-")}`}
            label={label}
            value={effectiveColor(path)}
            onChange={(v) => edit(path, v)}
            onClear={isInherited(path) ? () => edit(path, undefined) : undefined}
          />
        ))}
      </fieldset>

      {/* ---- Structure ---------------------------------------------------- */}
      <fieldset className={FIELDSET}>
        <legend className={LEGEND}>Structure</legend>
        {(
          [
            ["structure.axis", "Axis"],
            ["structure.grid", "Grid"],
            ["structure.rule", "Rule"],
          ] as const
        ).map(([path, label]) => (
          <ColorField
            key={path}
            id={`theme-${path.replace(".", "-")}`}
            label={label}
            value={effectiveColor(path)}
            onChange={(v) => edit(path, v)}
            onClear={isInherited(path) ? () => edit(path, undefined) : undefined}
          />
        ))}
      </fieldset>

      {/* ---- Series ------------------------------------------------------- */}
      <fieldset className={FIELDSET}>
        <legend className={LEGEND}>Series</legend>
        <ColorField
          id="theme-series-single"
          label="Single series"
          hint="Used when a chart has one series and no need to distinguish it."
          value={effectiveColor("series.single")}
          onChange={(v) => edit("series.single", v)}
          onClear={
            isInherited("series.single") ? () => edit("series.single", undefined) : undefined
          }
        />
      </fieldset>

      <CategoricalPaletteEditor
        colors={categorical}
        onChange={(next) => edit("series.categorical", next)}
      />

      {/* ---- Effective preview -------------------------------------------- */}
      <details className={EFFECTIVE}>
        <summary className={EFFECTIVE_SUMMARY}>Effective palette (read only)</summary>
        <p className={cn(HELP, "m-0")}>
          What the shell is painting right now, after inheritance. Your changes are already live —
          this is only the readout.
        </p>
        <div className={EFFECTIVE_GRID}>
          {effective?.series?.categorical?.length ? (
            effective.series.categorical.map((c, i) => (
              <ColorChip key={`${c}-${i}`} color={c} label={`${i + 1}`} />
            ))
          ) : (
            <span className={HELP}>
              No categorical palette — charts fall back to the dashboard default.
            </span>
          )}
        </div>
      </details>
    </div>
  );
};

function labelFor(presets: ThemePresetSummary[], id: string): string {
  return presets.find((p) => p.id === id)?.label ?? id;
}
