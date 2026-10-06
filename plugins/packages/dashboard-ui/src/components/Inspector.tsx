import React, { useState, useEffect } from "react";
import { Button, Label, Separator, cn } from "@loams-plugins/core/ui";
import { CONTROL, CONTROL_AREA, CONTROL_DISABLED, CONTROL_SELECT, GROUP, LABEL } from "./fields";
import { Widget, Dataset, fetchDatasets } from "../api";
import { WidgetThemeField } from "./WidgetThemeField";
import type { ThemePresetSummary, ThemeSelection } from "../theme/types";

interface InspectorProps {
  widget: Widget | null;
  onClose: () => void;
  onUpdate: (updatedWidget: Widget) => void;
  onDelete?: (widgetId: string) => void;
  /** Preset catalogue for the per-widget theme override. */
  themePresets?: ThemePresetSummary[];
  /** Dashboard-level theme, shown so the override's precedence is visible. */
  dashboardTheme?: ThemeSelection | null;
  themeCatalogError?: string | null;
}

/* ------------------------------------------------------------- fragments */

const SIDEBAR =
  "flex w-[320px] flex-col gap-5 border-l border-line bg-card p-6 shadow-[-2px_0_8px_rgba(0,0,0,0.04)]";

const HEADER = "flex items-center justify-between";

const TITLE = "text-[0.95rem] font-semibold text-ink";

/** The commit block is pinned to the bottom of the sidebar. */
const ACTIONS = "mt-auto flex flex-col gap-[0.6rem] pt-5";

const WIDE = "w-full";

export const Inspector: React.FC<InspectorProps> = ({
  widget,
  onClose,
  onUpdate,
  onDelete,
  themePresets = [],
  dashboardTheme = null,
  themeCatalogError = null,
}) => {
  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [title, setTitle] = useState("");
  const [kind, setKind] = useState<any>("line");
  const [xAxis, setXAxis] = useState("");
  const [yAxis, setYAxis] = useState("");
  const [flintJson, setFlintJson] = useState("");
  const [widgetTheme, setWidgetTheme] = useState<ThemeSelection | null>(null);

  useEffect(() => {
    fetchDatasets()
      .then(setDatasets)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!widget) return;
    setTitle(widget.flint?.title || (widget.chart?.optionOverrides as any)?.title?.text || "");
    if (widget.chart) {
      setKind(widget.chart.kind || "line");
      setXAxis(String(widget.chart.encode?.x || ""));
      setYAxis(
        String(
          Array.isArray(widget.chart.encode?.y)
            ? widget.chart.encode?.y[0]
            : widget.chart.encode?.y || "",
        ),
      );
    }
    if (widget.flint) {
      setFlintJson(JSON.stringify(widget.flint, null, 2));
      setWidgetTheme(widget.flint.theme_spec ?? null);
    } else {
      setWidgetTheme(null);
    }
  }, [widget]);

  if (!widget) return null;

  const currentDataset = datasets.find((d) => d.id === widget.data.datasetId);
  const columns = currentDataset?.columns || [];

  const handleSave = () => {
    const updated = JSON.parse(JSON.stringify(widget)) as Widget;

    if (widget.flint) {
      try {
        const parsedFlint = JSON.parse(flintJson);
        parsedFlint.title = title;
        // The dedicated control owns theme_spec; the raw JSON box may be stale.
        if (widgetTheme) {
          parsedFlint.theme_spec = widgetTheme;
        } else {
          delete parsedFlint.theme_spec;
        }
        updated.flint = parsedFlint;
      } catch (err: any) {
        alert("Invalid Flint JSON: " + err.message);
        return;
      }
    } else if (widget.chart) {
      updated.chart = {
        ...updated.chart,
        kind,
        encode: {
          x: xAxis || undefined,
          y: yAxis || undefined,
        },
        optionOverrides: {
          ...updated.chart?.optionOverrides,
          title: { text: title },
        },
      };
    }

    onUpdate(updated);
  };

  return (
    <aside className={SIDEBAR}>
      <div className={HEADER}>
        <h3 className={TITLE}>Widget Inspector</h3>
        <Button variant="outline" size="icon-sm" onClick={onClose} aria-label="Close the inspector">
          ✕
        </Button>
      </div>

      <div className={GROUP}>
        <Label className={LABEL} htmlFor="inspector-title">
          Title
        </Label>
        <input
          id="inspector-title"
          type="text"
          className={CONTROL}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Widget Title"
        />
      </div>

      <div className={GROUP}>
        <Label className={LABEL} htmlFor="inspector-type">
          Type
        </Label>
        <input
          id="inspector-type"
          type="text"
          className={cn(CONTROL, CONTROL_DISABLED)}
          value={widget.flint ? "Microsoft Flint (Semantic)" : `Native ECharts (${kind})`}
          disabled
        />
      </div>

      <div className={GROUP}>
        <Label className={LABEL} htmlFor="inspector-dataset">
          Dataset
        </Label>
        <input
          id="inspector-dataset"
          type="text"
          className={cn(CONTROL, CONTROL_DISABLED)}
          value={currentDataset?.table_name || `Dataset #${widget.data.datasetId}`}
          disabled
        />
      </div>

      {widget.chart && (
        <>
          <div className={GROUP}>
            <Label className={LABEL} htmlFor="inspector-kind">
              Chart Kind
            </Label>
            <select
              id="inspector-kind"
              className={CONTROL_SELECT}
              value={kind}
              onChange={(e) => setKind(e.target.value as any)}
            >
              <option value="line">Line Chart</option>
              <option value="bar">Bar Chart</option>
              <option value="pie">Pie Chart</option>
              <option value="scatter">Scatter Plot</option>
              <option value="heatmap">Heatmap</option>
              <option value="funnel">Funnel</option>
            </select>
          </div>

          <div className={GROUP}>
            <Label className={LABEL} htmlFor="inspector-x">
              X Axis / Dimension
            </Label>
            <select
              id="inspector-x"
              className={CONTROL_SELECT}
              value={xAxis}
              onChange={(e) => setXAxis(e.target.value)}
            >
              <option value="">Select column</option>
              {columns.map((c) => (
                <option key={c.column_name} value={c.column_name}>
                  {c.column_name} ({c.type})
                </option>
              ))}
            </select>
          </div>

          <div className={GROUP}>
            <Label className={LABEL} htmlFor="inspector-y">
              Y Axis / Measure
            </Label>
            <select
              id="inspector-y"
              className={CONTROL_SELECT}
              value={yAxis}
              onChange={(e) => setYAxis(e.target.value)}
            >
              <option value="">Select column</option>
              {columns.map((c) => (
                <option key={c.column_name} value={c.column_name}>
                  {c.column_name} ({c.type})
                </option>
              ))}
            </select>
          </div>
        </>
      )}

      {widget.flint && (
        <WidgetThemeField
          value={widget.flint.theme_spec}
          presets={themePresets}
          dashboardSelection={dashboardTheme}
          catalogError={themeCatalogError}
          onChange={setWidgetTheme}
        />
      )}

      {widget.flint && (
        <div className={GROUP}>
          <Label className={LABEL} htmlFor="inspector-flint">
            Flint Specification (JSON)
          </Label>
          <textarea
            id="inspector-flint"
            className={CONTROL_AREA}
            rows={10}
            value={flintJson}
            onChange={(e) => setFlintJson(e.target.value)}
          />
        </div>
      )}

      <Separator />

      <div className={cn(ACTIONS, "mt-0")}>
        <Button className={WIDE} onClick={handleSave}>
          Apply Changes
        </Button>
        {onDelete && (
          <Button
            type="button"
            variant="destructive-outline"
            className={WIDE}
            onClick={() => {
              onDelete(widget.id);
              onClose();
            }}
          >
            Delete Visualization
          </Button>
        )}
      </div>
    </aside>
  );
};
