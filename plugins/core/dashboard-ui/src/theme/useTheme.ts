import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { fetchTheme, fetchThemes, ThemesUnavailableError } from "./api";
import { inkToCssVars, mergeOverBase } from "./ink";
import type {
  ThemeInk,
  ThemePresetSummary,
  ThemeReportEntry,
  ThemeSelection,
  ThemeSpec,
} from "./types";

/**
 * Resolves the selected theme and projects its ink onto the shell.
 *
 * Two responsibilities kept together because they share one piece of state: the
 * ink you are editing IS the ink being previewed. There is no separate "draft"
 * concept — a customizer edit mutates local state and the same state change
 * repaints the page, which is what makes live preview fall out for free.
 *
 * Resolution is server-authoritative. `resolveThemeSpec` runs on the server, so
 * an unknown preset name comes back as `valid: false` and is rendered as an
 * error. flint is explicit that this is not a silent fallback, and neither is
 * this UI.
 */

export interface ThemeState {
  /** Presets from GET /api/themes. Empty when the endpoint is not available. */
  presets: ThemePresetSummary[];
  catalogLoading: boolean;
  /** Set when the theme endpoints are absent — degrade, never white-screen. */
  catalogError: string | null;

  /** The selection being previewed: preset id, custom spec, or null. */
  selection: ThemeSelection | null;
  /** The preset's own ink, before local overrides. Drives the inheritance hint. */
  baseInk: ThemeInk | undefined;
  /** Resolved/merged ink driving the shell's CSS custom properties. */
  ink: ThemeInk | undefined;
  /** flint's own downgrade notes for the current selection. */
  report: ThemeReportEntry[];
  /** Non-null when the selection cannot resolve. Never silently ignored. */
  error: string | null;
  resolving: boolean;

  select: (selection: ThemeSelection | null) => void;
  reloadCatalog: () => void;
}

export interface UseThemeOptions {
  /** Selection persisted on the dashboard spec, if any. */
  initialSelection: ThemeSelection | null | undefined;
}

export function useTheme({ initialSelection }: UseThemeOptions): ThemeState {
  const [presets, setPresets] = useState<ThemePresetSummary[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(true);
  const [catalogError, setCatalogError] = useState<string | null>(null);

  const [selection, setSelection] = useState<ThemeSelection | null>(initialSelection ?? null);
  const [resolvedSpec, setResolvedSpec] = useState<ThemeSpec | undefined>(undefined);
  const [report, setReport] = useState<ThemeReportEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);

  // Guards against a slow resolve landing after the user has moved on.
  const requestRef = useRef(0);
  // Once the user picks something, their choice wins over the persisted value
  // for the rest of the session — a late-arriving spec must not undo it.
  const userPickedRef = useRef(false);

  // The dashboard arrives after this hook mounts, so the persisted selection has
  // to be adopted once it shows up. Seeded exactly once.
  useEffect(() => {
    if (userPickedRef.current) return;
    if (initialSelection === null || initialSelection === undefined) return;
    setSelection(initialSelection);
  }, [initialSelection]);

  const loadCatalog = useCallback(() => {
    setCatalogLoading(true);
    fetchThemes()
      .then((rows) => {
        setPresets(rows);
        setCatalogError(null);
        setCatalogLoading(false);
      })
      .catch((err: unknown) => {
        setPresets([]);
        setCatalogError(
          err instanceof ThemesUnavailableError
            ? err.message
            : err instanceof Error
              ? err.message
              : "Themes are unavailable.",
        );
        setCatalogLoading(false);
      });
  }, []);

  useEffect(() => {
    loadCatalog();
  }, [loadCatalog]);

  /** The preset this selection depends on, if any. */
  const extendsId = useMemo(() => {
    if (selection && typeof selection === "object" && typeof selection.extends === "string") {
      return selection.extends;
    }
    return null;
  }, [selection]);

  /** The preset name to ask the server about, if the selection names one. */
  const presetId = useMemo(() => {
    if (typeof selection === "string" && selection.length > 0) return selection;
    return extendsId;
  }, [selection, extendsId]);

  useEffect(() => {
    if (!presetId) {
      // A bare custom spec with no `extends` needs no server round-trip.
      requestRef.current += 1;
      setResolvedSpec(undefined);
      setReport([]);
      setError(null);
      setResolving(false);
      return;
    }

    const token = requestRef.current + 1;
    requestRef.current = token;
    setResolving(true);

    fetchTheme(presetId)
      .then((resolution) => {
        if (requestRef.current !== token) return;
        if (!resolution.valid) {
          // flint treats an unknown house as an error. Say so; do not paint.
          setResolvedSpec(undefined);
          setReport(resolution.report);
          setError(
            `Theme "${presetId}" could not be resolved. Flint reports an unknown preset name as an error rather than falling back, so nothing is being themed.`,
          );
          setResolving(false);
          return;
        }
        setResolvedSpec(resolution.spec);
        setReport(resolution.report);
        setError(null);
        setResolving(false);
      })
      .catch((err: unknown) => {
        if (requestRef.current !== token) return;
        setResolvedSpec(undefined);
        setReport([]);
        setError(err instanceof Error ? err.message : "Theme could not be resolved.");
        setResolving(false);
      });
  }, [presetId]);

  /**
   * The effective ink: the resolved base from the server merged with any local
   * overrides. Nested objects merge and arrays replace, matching flint — so
   * typing one new hex into an `extends` theme changes only that field.
   */
  const ink = useMemo<ThemeInk | undefined>(() => {
    if (error) return undefined;
    if (!selection) return undefined;

    if (typeof selection === "string") {
      return resolvedSpec?.ink;
    }
    return mergeOverBase(resolvedSpec?.ink, selection.ink);
  }, [selection, resolvedSpec, error]);

  /* ------------------------------------------------ live preview: CSS vars */

  // Written imperatively to the root element rather than through a `style`
  // prop: the vars must land on :root so `body`, the header, the inspector and
  // widget cards — which are all descendants — derive from one place. Clearing
  // on unmount returns the app to its stylesheet defaults.
  useEffect(() => {
    const root = document.documentElement;
    const vars = inkToCssVars(ink);
    for (const [name, value] of vars) {
      root.style.setProperty(name, value);
    }
    root.dataset.themed = ink ? "true" : "false";
    return () => {
      for (const [name] of vars) root.style.removeProperty(name);
    };
  }, [ink]);

  const select = useCallback((next: ThemeSelection | null) => {
    userPickedRef.current = true;
    setSelection(next);
    setError(null);
    setReport([]);
  }, []);

  return {
    presets,
    catalogLoading,
    catalogError,
    selection,
    baseInk: resolvedSpec?.ink,
    ink,
    report,
    error,
    resolving,
    select,
    reloadCatalog: loadCatalog,
  };
}
