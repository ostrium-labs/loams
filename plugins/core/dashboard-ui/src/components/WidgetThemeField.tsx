import React from "react";
import { Label, Separator, cn } from "@loams-core/host/ui";
import { DefaultThemeIcon, ThemeIcon } from "./ThemeIcon";
import { CONTROL_AREA, CONTROL_SELECT, GROUP, HELP, LABEL } from "./fields";
import { describeSelection } from "../theme/ink";
import type { ThemePresetSummary, ThemeSelection } from "../theme/types";

interface WidgetThemeFieldProps {
  /** Currently stored on `widget.chartSpec.theme_spec`, if any. */
  value: ThemeSelection | undefined;
  presets: ThemePresetSummary[];
  /** What the dashboard-level theme currently is, for the copy below. */
  dashboardSelection: ThemeSelection | null;
  catalogError: string | null;
  onChange: (next: ThemeSelection | null) => void;
}

/* ------------------------------------------------------------- fragments */

const CUSTOM_EDITOR = "mt-[0.4rem] flex flex-col gap-[0.3rem]";

const PREVIEW =
  "flex items-center gap-2 rounded border border-line-subtle bg-page px-[0.6rem] py-[0.45rem]";

const PREVIEW_ICON = "inline-flex shrink-0 items-center justify-center text-ink-body";

const PREVIEW_TEXT = "text-[0.74rem] text-ink-body";

const PRECEDENCE = "mt-[0.1rem]";

/**
 * Per-widget theme override.
 *
 * Precedence is the thing worth being loud about: a widget's own
 * `chartSpec.theme_spec` WINS over the dashboard theme. That is deliberate — one
 * chart can be the exception — but it is surprising if the copy does not say so,
 * because a widget can look un-themed while the dashboard around it is themed,
 * and the obvious conclusion ("the theme is broken") is the wrong one.
 *
 * Only chart-specs widgets carry `theme_spec`; a native-ECharts widget has no such
 * field, so the control is not offered for one.
 */
export const WidgetThemeField: React.FC<WidgetThemeFieldProps> = ({
  value,
  presets,
  dashboardSelection,
  catalogError,
  onChange,
}) => {
  const current = value ?? null;

  return (
    <div className={GROUP}>
      <Label className={LABEL} htmlFor="widget-theme-select">
        Theme override
      </Label>

      <select
        id="widget-theme-select"
        className={CONTROL_SELECT}
        value={typeof current === "string" ? `preset:${current}` : current ? "custom" : "inherit"}
        onChange={(e) => {
          const v = e.target.value;
          if (v === "inherit") onChange(null);
          else if (v === "custom") onChange({ extends: undefined });
          else onChange(v.slice("preset:".length));
        }}
      >
        <option value="inherit">
          Use dashboard theme ({describeSelection(dashboardSelection)})
        </option>
        {presets.map((p) => (
          <option key={p.id} value={`preset:${p.id}`}>
            {p.label}
          </option>
        ))}
        <option value="custom">Custom theme…</option>
      </select>

      {current && typeof current === "object" && (
        <div className={CUSTOM_EDITOR}>
          <textarea
            className={CONTROL_AREA}
            rows={5}
            spellCheck={false}
            aria-label="Custom theme spec JSON for this widget"
            value={JSON.stringify(current, null, 2)}
            onChange={(e) => {
              try {
                const parsed = JSON.parse(e.target.value) as ThemeSelection;
                onChange(parsed);
              } catch {
                // Deliberately inert: the invalid text stays visible in the box
                // and the last valid spec keeps applying, so a half-typed JSON
                // body never blanks the chart.
              }
            }}
          />
          <span className={HELP}>
            Leave valid JSON to apply it. Editing here overrides the dashboard theme for this widget
            only.
          </span>
        </div>
      )}

      <Separator />

      <div className={PREVIEW}>
        {current === null ? (
          <>
            <span className={PREVIEW_ICON} aria-hidden="true">
              <DefaultThemeIcon size={18} />
            </span>
            <span className={PREVIEW_TEXT}>
              Inheriting the dashboard theme
              {dashboardSelection === null && " (which is the default theme)"}.
            </span>
          </>
        ) : typeof current === "string" ? (
          <>
            <span className={PREVIEW_ICON} aria-hidden="true">
              <ThemeIcon
                svg={presets.find((p) => p.id === current)?.icon ?? ""}
                label={`${current} preview`}
                size={18}
              />
            </span>
            <span className={PREVIEW_TEXT}>
              Overridden with <strong>{describeSelection(current)}</strong>.
            </span>
          </>
        ) : (
          <span className={PREVIEW_TEXT}>
            Overridden with a custom spec
            {current.extends ? ` extending “${current.extends}”` : ""}.
          </span>
        )}
      </div>

      <p className={cn(HELP, PRECEDENCE, "m-0")}>
        <strong>Precedence:</strong> this widget&apos;s override wins over the dashboard theme.
        Leave it on <em>Use dashboard theme</em> to follow the dashboard.
      </p>

      {catalogError && (
        <p className={cn(HELP, "m-0")}>
          The theme catalogue is unavailable, so only the dashboard theme and a hand-written spec
          can be chosen here.
        </p>
      )}
    </div>
  );
};
