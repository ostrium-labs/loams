import React, { useEffect, useState } from "react";
import { ThemeIcon, DefaultThemeIcon } from "./ThemeIcon";
import { ThemeCustomizer } from "./ThemeCustomizer";
import { ThemeReportPanel } from "./ThemeReportPanel";
import { describeSelection } from "../theme/ink";
import type { ThemeSpec, ThemeSelection } from "../theme/types";
import type { ThemeState } from "../theme/useTheme";
import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
  Button,
  Separator,
  Skeleton,
  cn,
} from "@loams-core/host/ui";
import { HELP } from "./fields";

interface ThemePickerSheetProps {
  isOpen: boolean;
  onClose: () => void;
  /** Selection last persisted on the dashboard spec. */
  savedSelection: ThemeSelection | null;
  theme: ThemeState;
  onSave: (selection: ThemeSelection | null) => void;
}

type Tab = "themes" | "customize";

/* ------------------------------------------------------------- fragments */

const BACKDROP =
  "fixed inset-0 z-2000 flex justify-end bg-[rgba(20,20,20,0.45)] animate-sheet-backdrop-in";

const SHEET =
  "flex h-screen w-[640px] max-w-[92vw] flex-col bg-card shadow-[-4px_0_24px_rgba(0,0,0,0.16)] [transform:translate3d(0,0,0)] animate-sheet-in";

const HEADER = "flex items-start justify-between border-b border-line bg-card px-6 pt-5 pb-4";

const TITLE_GROUP = "flex flex-col gap-[0.2rem]";

const TITLE = "text-[1.15rem] font-semibold text-ink";

const TABS = "flex gap-1 border-b border-line px-6 pt-[0.6rem]";

const TAB =
  "cursor-pointer border-none border-b-2 border-transparent bg-transparent px-3 py-2 font-sans text-[0.82rem] font-semibold text-ink-body hover:text-ink";

const TAB_ACTIVE = "border-primary-solid text-primary-ink";

const CONTENT = "overflow-y-auto";

const BODY = "flex flex-col gap-[1.1rem] px-6 py-[1.15rem]";

const LOADING =
  "flex items-center justify-center gap-[0.7rem] px-4 py-10 text-[0.85rem] text-ink-muted";

const PRESET_GRID = "grid grid-cols-2 gap-[0.6rem]";

const PRESET_CARD =
  "flex items-center gap-[0.7rem] rounded border border-line bg-card p-[0.6rem_0.7rem] text-left font-sans cursor-pointer transition-[border-color,box-shadow] duration-150 hover:border-primary";

/*
 * Selected carries a border AND an inset ring AND a tint, not colour alone: a
 * themed surface can make two tints of the accent indistinguishable, and the
 * ring survives any palette.
 */
const PRESET_CARD_ACTIVE =
  "border-primary-solid bg-primary-subtle shadow-[inset_0_0_0_1px_var(--primary-solid)]";

const PRESET_ICON =
  "flex size-8 shrink-0 items-center justify-center overflow-hidden rounded border border-line-subtle bg-page text-ink-body";

const PRESET_META = "flex min-w-0 flex-col gap-[0.15rem]";

const PRESET_NAME = "text-[0.82rem] font-semibold text-ink";

const PRESET_DESC = "text-[0.7rem] leading-[1.3] text-ink-body";

const FOOTER =
  "flex items-center justify-between gap-4 border-t border-line bg-card px-6 py-[0.85rem]";

const FOOTER_ACTIONS = "flex shrink-0 justify-end gap-3";

/**
 * The theme sheet: pick a house, or edit one.
 *
 * One sheet with two tabs rather than two surfaces, because the two are the
 * same decision — the picker sets the selection, the customizer edits it, and
 * both preview live against the same state.
 */
export const ThemePickerSheet: React.FC<ThemePickerSheetProps> = ({
  isOpen,
  onClose,
  savedSelection,
  theme,
  onSave,
}) => {
  const [tab, setTab] = useState<Tab>("themes");

  // The spec being edited, kept as a draft so Revert has something to return to.
  const [draft, setDraft] = useState<ThemeSpec>(() =>
    theme.selection && typeof theme.selection === "object"
      ? (theme.selection as ThemeSpec)
      : { extends: theme.selection ?? undefined },
  );

  // Re-seed the draft each time the sheet opens on the saved value.
  useEffect(() => {
    if (!isOpen) return;
    setDraft(
      savedSelection && typeof savedSelection === "object"
        ? (savedSelection as ThemeSpec)
        : { extends: savedSelection ?? undefined },
    );
    setTab("themes");
  }, [isOpen, savedSelection]);

  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  /**
   * The one place a selection changes: the draft (what Revert restores and what
   * the customizer edits) and the live preview move together, or the two drift
   * and Revert reverts something the user never saw.
   */
  const applySelection = (next: ThemeSelection | null) => {
    setDraft(
      next && typeof next === "object" ? (next as ThemeSpec) : { extends: next ?? undefined },
    );
    theme.select(next);
  };

  const extendsId = draft.extends ?? null;
  const customDraft: ThemeSpec = { ...draft, ink: draft.ink };

  const isCustom = theme.selection !== null && typeof theme.selection === "object";
  const dirty =
    JSON.stringify(stripUndefined(draft)) !==
    JSON.stringify(
      stripUndefined(
        savedSelection && typeof savedSelection === "object"
          ? (savedSelection as ThemeSpec)
          : { extends: savedSelection ?? undefined },
      ),
    );

  const presetBase = theme.presets.find((p) => p.id === extendsId);

  return (
    <div
      className={BACKDROP}
      role="presentation"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className={SHEET} role="dialog" aria-modal="true" aria-label="Dashboard theme">
        <div className={HEADER}>
          <div className={TITLE_GROUP}>
            <span className={TITLE}>Theme</span>
            <span className={HELP}>
              Sets the house for the whole dashboard. Charts pick it up from the server; the shell
              around them repaints instantly.
            </span>
          </div>
          <Button variant="ghost" size="icon-sm" onClick={onClose} aria-label="Close theme picker">
            ✕
          </Button>
        </div>

        <div className={TABS} role="tablist" aria-label="Theme mode">
          <button
            type="button"
            role="tab"
            id="theme-tab-themes"
            aria-selected={tab === "themes"}
            aria-controls="theme-panel-themes"
            className={cn(TAB, tab === "themes" && TAB_ACTIVE)}
            onClick={() => setTab("themes")}
          >
            Themes
          </button>
          <button
            type="button"
            role="tab"
            id="theme-tab-customize"
            aria-selected={tab === "customize"}
            aria-controls="theme-panel-customize"
            className={cn(TAB, tab === "customize" && TAB_ACTIVE)}
            onClick={() => setTab("customize")}
          >
            Customise
          </button>
        </div>

        <Separator />

        <div className={CONTENT}>
          {theme.catalogError && (
            <div className="px-6 pt-4">
              {/*
               * `Alert` splits its children by slot, so the title, the
               * description and the retry have to arrive as `AlertTitle`,
               * `AlertDescription` and `AlertAction` respectively -- a wrapper
               * `div` in between would be read as the icon.
               */}
              <Alert variant="warning">
                <AlertTitle>Theme catalogue unavailable</AlertTitle>
                <AlertDescription>{theme.catalogError}</AlertDescription>
                <AlertAction>
                  <Button variant="outline" size="xs" onClick={theme.reloadCatalog}>
                    Try again
                  </Button>
                </AlertAction>
              </Alert>
            </div>
          )}

          {tab === "themes" && (
            <div
              id="theme-panel-themes"
              role="tabpanel"
              aria-labelledby="theme-tab-themes"
              className={BODY}
            >
              {theme.catalogLoading ? (
                <div className={LOADING} role="status">
                  <Skeleton className="h-5 w-40" />
                  <span>Loading themes…</span>
                </div>
              ) : (
                <div className={PRESET_GRID} aria-label="Theme presets">
                  <button
                    type="button"
                    aria-pressed={theme.selection === null}
                    className={cn(PRESET_CARD, theme.selection === null && PRESET_CARD_ACTIVE)}
                    onClick={() => applySelection(null)}
                  >
                    <span className={PRESET_ICON}>
                      <DefaultThemeIcon />
                    </span>
                    <span className={PRESET_META}>
                      <span className={PRESET_NAME}>Default theme</span>
                      <span className={PRESET_DESC}>No house. The built-in neutral theme.</span>
                    </span>
                  </button>

                  {theme.presets.map((preset) => {
                    const active = theme.selection === preset.id;
                    return (
                      <button
                        key={preset.id}
                        type="button"
                        aria-pressed={active}
                        className={cn(PRESET_CARD, active && PRESET_CARD_ACTIVE)}
                        onClick={() => applySelection(preset.id)}
                      >
                        <span className={PRESET_ICON}>
                          <ThemeIcon svg={preset.icon} label={`${preset.label} preview`} />
                        </span>
                        <span className={PRESET_META}>
                          <span className={PRESET_NAME}>{preset.label}</span>
                          <span className={PRESET_DESC}>{preset.description}</span>
                        </span>
                      </button>
                    );
                  })}
                </div>
              )}

              <ThemeReportPanel
                report={theme.report}
                error={theme.error}
                resolving={theme.resolving}
              />

              {isCustom && (
                <Button variant="outline" size="sm" onClick={() => setTab("customize")}>
                  Edit this custom theme
                </Button>
              )}
            </div>
          )}

          {tab === "customize" && (
            <div
              id="theme-panel-customize"
              role="tabpanel"
              aria-labelledby="theme-tab-customize"
              className={BODY}
            >
              {presetBase && (
                <p className={cn(HELP, "m-0")}>
                  <strong>{presetBase.label} asks for:</strong> {presetBase.description}
                </p>
              )}

              <ThemeCustomizer
                spec={customDraft}
                baseInk={extendsId ? theme.baseInk : undefined}
                extendsId={extendsId}
                presets={theme.presets}
                // Editing the spec pushes straight into live state: that is
                // the preview path, with no save round-trip in between.
                onChange={(next) => applySelection(next)}
              />

              <ThemeReportPanel
                report={theme.report}
                error={theme.error}
                resolving={theme.resolving}
              />
            </div>
          )}
        </div>

        <div className={FOOTER}>
          <div className="min-w-0">
            <span className={HELP}>
              Current: <strong>{describeSelection(theme.selection)}</strong>
              {dirty && " — unsaved"}
            </span>
          </div>
          <div className={FOOTER_ACTIONS}>
            <Button
              variant="outline"
              disabled={!dirty}
              onClick={() => applySelection(savedSelection ?? null)}
            >
              Revert
            </Button>
            <Button disabled={!dirty} onClick={() => onSave(theme.selection)}>
              Save theme
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
};

/** JSON.stringify drops undefined values already; this just narrows the type. */
function stripUndefined(value: unknown): unknown {
  return JSON.parse(JSON.stringify(value ?? null)) as unknown;
}
