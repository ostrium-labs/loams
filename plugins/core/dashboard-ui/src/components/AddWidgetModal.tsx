import React, { useState, useEffect } from "react";
import { Button, Label, cn } from "@loams-core/host/ui";
import { CONTROL, CONTROL_SELECT, GROUP, LABEL } from "./fields";
import { Widget, Dataset, fetchDatasets } from "../api";

interface AddWidgetModalProps {
  isOpen: boolean;
  onClose: () => void;
  onAdd: (widget: Widget) => void;
}

/* ------------------------------------------------------------- fragments */

/*
 * This component is not mounted anywhere -- `ChartPickerSheet` is the path that
 * adds a widget. Its selectors used to be `.modal-overlay`, `.modal-content`
 * and friends, none of which had a rule in `styles.css`, so the modal rendered
 * as unstyled stacked divs. Converted here so that if it is ever mounted it is
 * a real dialog rather than a surprise.
 */
const OVERLAY =
  "fixed inset-0 z-2000 flex items-center justify-center bg-[rgba(20,20,20,0.45)] animate-sheet-backdrop-in";

const CONTENT =
  "flex max-h-[90vh] w-[520px] max-w-[92vw] flex-col overflow-hidden rounded-lg bg-card shadow-[0_8px_32px_rgba(0,0,0,0.24)]";

const HEADER = "flex items-center justify-between border-b border-line px-6 py-4";

const TITLE = "text-[1.05rem] font-semibold text-ink";

const BODY = "flex flex-col gap-4 overflow-y-auto px-6 py-5";

const FOOTER = "flex justify-end gap-3 border-t border-line bg-page px-6 py-4";

const MODE_ROW = "flex gap-2";

/**
 * A two-way choice rendered as a pressed/unpressed pair.
 *
 * `aria-pressed` rather than a radio group: the two modes are not a value in a
 * closed set the form submits, they are which compiler runs.
 */
const MODE = "flex-1 cursor-pointer disabled:cursor-not-allowed disabled:opacity-65";

const MODE_ON = "border-primary-solid bg-primary-solid text-primary-foreground";

const MODE_OFF =
  "border-line bg-card text-ink-body hover:border-ink-subtle hover:bg-page hover:text-ink";

const HALF = "flex-1";

export const AddWidgetModal: React.FC<AddWidgetModalProps> = ({ isOpen, onClose, onAdd }) => {
  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [selectedDatasetId, setSelectedDatasetId] = useState<number>(1);
  const [mode, setMode] = useState<"flint" | "native">("flint");
  const [title, setTitle] = useState("");
  const [chartKind, setChartKind] = useState<any>("bar");
  const [xAxis, setXAxis] = useState("");
  const [yAxis, setYAxis] = useState("");

  useEffect(() => {
    if (isOpen) {
      fetchDatasets()
        .then((data) => {
          setDatasets(data);
          if (data.length > 0) setSelectedDatasetId(data[0].id);
        })
        .catch(() => {});
    }
  }, [isOpen]);

  if (!isOpen) return null;

  const currentDataset = datasets.find((d) => d.id === selectedDatasetId);
  const columns = currentDataset?.columns || [];

  const handleAdd = () => {
    const id = "widget-" + Math.random().toString(36).substring(2, 9);

    let widget: Widget;

    if (mode === "flint") {
      widget = {
        id,
        type: "chart",
        data: { source: "superset", datasetId: selectedDatasetId },
        flint: {
          chartType:
            chartKind === "line" ? "Line Chart" : chartKind === "pie" ? "Pie Chart" : "Bar Chart",
          title: title || "New Flint Chart",
          encodings: {
            x: { field: xAxis || columns[0]?.column_name || "x" },
            y: { field: yAxis || columns[1]?.column_name || "y" },
          },
          baseSize: { width: 400, height: 280 },
        },
      };
    } else {
      widget = {
        id,
        type: "chart",
        data: { source: "superset", datasetId: selectedDatasetId },
        chart: {
          kind: chartKind,
          encode: {
            x: xAxis || columns[0]?.column_name,
            y: yAxis || columns[1]?.column_name,
            value: yAxis || columns[1]?.column_name,
          },
          optionOverrides: {
            title: { text: title || "New Native Chart" },
          },
        },
      };
    }

    onAdd(widget);
    onClose();
  };

  return (
    <div className={OVERLAY} onClick={onClose}>
      <div
        className={CONTENT}
        role="dialog"
        aria-modal="true"
        aria-label="Add Dashboard Widget"
        onClick={(e) => e.stopPropagation()}
      >
        <div className={HEADER}>
          <h2 className={TITLE}>Add Dashboard Widget</h2>
          <Button
            variant="outline"
            size="icon-sm"
            onClick={onClose}
            aria-label="Close the add-widget dialog"
          >
            ✕
          </Button>
        </div>

        <div className={BODY}>
          <div className={GROUP}>
            <Label className={LABEL} htmlFor="add-widget-title">
              Widget Title
            </Label>
            <input
              id="add-widget-title"
              type="text"
              className={CONTROL}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Revenue by Quarter"
            />
          </div>

          <div className={GROUP}>
            <span className={LABEL} id="add-widget-mode-label">
              Specification Mode
            </span>
            <div className={MODE_ROW} role="group" aria-labelledby="add-widget-mode-label">
              <Button
                variant="outline"
                aria-pressed={mode === "flint"}
                className={cn(MODE, mode === "flint" ? MODE_ON : MODE_OFF)}
                onClick={() => setMode("flint")}
              >
                Microsoft Flint (Semantic Spec)
              </Button>
              <Button
                variant="outline"
                aria-pressed={mode === "native"}
                className={cn(MODE, mode === "native" ? MODE_ON : MODE_OFF)}
                onClick={() => setMode("native")}
              >
                Native ECharts
              </Button>
            </div>
          </div>

          <div className={GROUP}>
            <Label className={LABEL} htmlFor="add-widget-dataset">
              Superset Dataset
            </Label>
            <select
              id="add-widget-dataset"
              className={CONTROL_SELECT}
              value={selectedDatasetId}
              onChange={(e) => setSelectedDatasetId(Number.parseInt(e.target.value, 10))}
            >
              {datasets.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.table_name} ({d.database.database_name})
                </option>
              ))}
            </select>
          </div>

          <div className={GROUP}>
            <Label className={LABEL} htmlFor="add-widget-kind">
              Chart Kind
            </Label>
            <select
              id="add-widget-kind"
              className={CONTROL_SELECT}
              value={chartKind}
              onChange={(e) => setChartKind(e.target.value as any)}
            >
              <option value="bar">Bar Chart</option>
              <option value="line">Line Chart</option>
              <option value="pie">Pie Chart</option>
              <option value="scatter">Scatter Plot</option>
            </select>
          </div>

          <div className="flex gap-4">
            <div className={cn(GROUP, HALF)}>
              <Label className={LABEL} htmlFor="add-widget-x">
                X Dimension / Category
              </Label>
              <select
                id="add-widget-x"
                className={CONTROL_SELECT}
                value={xAxis}
                onChange={(e) => setXAxis(e.target.value)}
              >
                <option value="">Select column</option>
                {columns.map((c) => (
                  <option key={c.column_name} value={c.column_name}>
                    {c.column_name}
                  </option>
                ))}
              </select>
            </div>

            <div className={cn(GROUP, HALF)}>
              <Label className={LABEL} htmlFor="add-widget-y">
                Y Measure / Value
              </Label>
              <select
                id="add-widget-y"
                className={CONTROL_SELECT}
                value={yAxis}
                onChange={(e) => setYAxis(e.target.value)}
              >
                <option value="">Select column</option>
                {columns.map((c) => (
                  <option key={c.column_name} value={c.column_name}>
                    {c.column_name}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </div>

        <div className={FOOTER}>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={handleAdd}>Create Widget</Button>
        </div>
      </div>
    </div>
  );
};
