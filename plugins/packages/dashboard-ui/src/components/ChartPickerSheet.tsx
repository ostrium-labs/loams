import React, { useState, useEffect, useMemo } from "react";
import { Widget, Dataset, fetchDatasets } from "../api";
import { CHART_CATALOG, CHART_CATEGORIES, ChartCatalogItem } from "../data/chartCatalog";
import {
  Button,
  Empty,
  EmptyDescription,
  EmptyTitle,
  InlineButton,
  Kbd,
  Label,
  cn,
} from "@loams-plugins/core/ui";
import { CONTROL, CONTROL_SELECT, GROUP, HELP, LABEL } from "./fields";

interface ChartPickerSheetProps {
  isOpen: boolean;
  onClose: () => void;
  onAdd: (widget: Widget) => void;
}

/* ------------------------------------------------------------- fragments */

const BACKDROP =
  "fixed inset-0 z-2000 flex justify-end bg-[rgba(20,20,20,0.45)] animate-sheet-backdrop-in";

const SHEET =
  "flex h-screen w-[580px] max-w-[92vw] flex-col bg-card shadow-[-4px_0_24px_rgba(0,0,0,0.16)] [transform:translate3d(0,0,0)] animate-sheet-in";

const HEADER = "flex items-start justify-between border-b border-line bg-card px-6 pt-5 pb-4";

const TITLE_GROUP = "flex flex-col gap-[0.2rem]";

const TITLE = "text-[1.15rem] font-semibold text-ink";

const SUBTITLE = "text-[0.78rem] text-ink-body";

const CLOSE = "shrink-0 text-[0.78rem] text-ink-body";

const CONTENT = "flex flex-1 flex-col overflow-hidden";

const SEARCH_BAR = "relative flex items-center px-6 pt-[0.85rem] pb-2";

const SEARCH_INPUT = `${CONTROL} pr-24`;

const SEARCH_CLEAR =
  "absolute right-[3.6rem] cursor-pointer border-none bg-transparent text-[0.85rem] text-ink-muted";

const CATEGORY_PILLS =
  "flex gap-[0.4rem] overflow-x-auto border-b border-line-subtle px-6 pt-1 pb-3 whitespace-nowrap [scrollbar-width:thin]";

const CATEGORY_PILL =
  "shrink-0 cursor-pointer rounded-[14px] border border-line bg-page px-[0.65rem] py-[0.25rem] font-sans text-[0.74rem] text-ink-body transition-all duration-150 hover:bg-line-subtle hover:text-ink";

const CATEGORY_PILL_ACTIVE = "border-primary bg-primary font-semibold text-primary-foreground";

const CHART_GRID = "grid flex-1 grid-cols-2 content-start gap-3 overflow-y-auto px-6 py-4";

const CHART_CARD =
  "relative flex cursor-pointer gap-[0.65rem] rounded border border-line bg-card p-3 transition-all duration-150 hover:-translate-y-px hover:border-primary hover:shadow-[0_2px_6px_var(--primary-subtle)]";

const CHART_CARD_ICON = "flex size-[38px] shrink-0 items-center justify-center rounded bg-page";

const CHART_CARD_INFO = "flex min-w-0 flex-col gap-[0.1rem]";

const CHART_CARD_NAME = "truncate text-[0.82rem] font-semibold text-ink";

const CHART_CARD_CATEGORY = "text-[0.68rem] font-medium text-primary";

const CHART_CARD_DESC =
  "mt-[0.15rem] line-clamp-2 overflow-hidden text-[0.7rem] leading-[1.25] text-ink-body";

const FORM = "flex flex-col gap-[1.15rem] overflow-y-auto px-6 pt-5 pb-8";

const BACK_LINK =
  "self-start p-0 font-semibold text-primary no-underline hover:text-primary hover:underline";

const BANNER =
  "flex items-center gap-[0.85rem] rounded border border-primary-border bg-primary-subtle p-[0.85rem]";

const BANNER_ICON =
  "flex size-11 shrink-0 items-center justify-center rounded bg-card shadow-[0_1px_3px_rgba(0,0,0,0.08)]";

const BANNER_TEXT = "flex flex-col gap-[0.15rem]";

const BANNER_TITLE = "text-[0.92rem] font-semibold text-primary-ink";

const BANNER_DESC = "text-[0.74rem] text-ink";

const THEME_TAG =
  "flex items-center gap-[0.4rem] rounded bg-page px-3 py-[0.45rem] text-[0.74rem] text-ink-body";

const ACTIONS = "mt-4 flex justify-end gap-3";

// Mini SVG pattern icons for the 63 chart types
const ChartTypeIcon: React.FC<{ iconType: string }> = ({ iconType }) => {
  switch (iconType) {
    case "bar":
    case "clustered-bar":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <rect x="3" y="4" width="16" height="4" rx="1" fill="#118dff" />
          <rect x="3" y="10" width="12" height="4" rx="1" fill="#118dff" opacity="0.8" />
          <rect x="3" y="16" width="18" height="4" rx="1" fill="#118dff" opacity="0.6" />
        </svg>
      );
    case "stacked-bar":
    case "100-stacked-bar":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <rect x="3" y="5" width="8" height="5" rx="1" fill="#118dff" />
          <rect x="12" y="5" width="6" height="5" rx="1" fill="#e66c37" />
          <rect x="3" y="14" width="11" height="5" rx="1" fill="#118dff" />
          <rect x="15" y="14" width="5" height="5" rx="1" fill="#e66c37" />
        </svg>
      );
    case "column":
    case "clustered-column":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <rect x="4" y="9" width="3.5" height="11" rx="1" fill="#118dff" />
          <rect x="10" y="4" width="3.5" height="16" rx="1" fill="#118dff" />
          <rect x="16.5" y="12" width="3.5" height="8" rx="1" fill="#118dff" />
        </svg>
      );
    case "stacked-column":
    case "100-stacked-column":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <rect x="4" y="13" width="4" height="7" rx="1" fill="#118dff" />
          <rect x="4" y="6" width="4" height="6" rx="1" fill="#e66c37" />
          <rect x="11" y="9" width="4" height="11" rx="1" fill="#118dff" />
          <rect x="11" y="3" width="4" height="5" rx="1" fill="#e66c37" />
          <rect x="18" y="14" width="4" height="6" rx="1" fill="#118dff" />
          <rect x="18" y="8" width="4" height="5" rx="1" fill="#e66c37" />
        </svg>
      );
    case "line":
    case "spline":
    case "step-line":
    case "multi-line":
      return (
        <svg
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          stroke="#118dff"
          strokeWidth="2.2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <polyline points="3 17 8 11 14 15 21 6" />
          <circle cx="8" cy="11" r="1.5" fill="#118dff" />
          <circle cx="14" cy="15" r="1.5" fill="#118dff" />
          <circle cx="21" cy="6" r="1.5" fill="#118dff" />
        </svg>
      );
    case "area":
    case "stacked-area":
    case "100-stacked-area":
    case "range-area":
    case "streamgraph":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <path d="M3 18 L3 12 L8 7 L14 11 L21 5 L21 18 Z" fill="#118dff" fillOpacity="0.25" />
          <polyline
            points="3 12 8 7 14 11 21 5"
            stroke="#118dff"
            strokeWidth="2"
            strokeLinecap="round"
          />
        </svg>
      );
    case "pie":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <circle
            cx="12"
            cy="12"
            r="9"
            fill="#118dff"
            fillOpacity="0.2"
            stroke="#118dff"
            strokeWidth="1.5"
          />
          <path d="M12 12 L12 3 A9 9 0 0 1 20.5 15.5 Z" fill="#118dff" />
        </svg>
      );
    case "donut":
    case "semi-donut":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <circle
            cx="12"
            cy="12"
            r="8.5"
            fill="none"
            stroke="#118dff"
            strokeWidth="4"
            strokeDasharray="38 15"
          />
          <circle cx="12" cy="12" r="4.5" fill="#ffffff" />
        </svg>
      );
    case "funnel":
    case "pyramid-funnel":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <polygon points="3 4 21 4 15 13 15 19 9 19 9 13" fill="#118dff" fillOpacity="0.8" />
        </svg>
      );
    case "treemap":
    case "heatmap-matrix":
    case "calendar-heatmap":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <rect x="3" y="3" width="10" height="11" rx="1" fill="#118dff" />
          <rect x="14" y="3" width="7" height="6" rx="1" fill="#12239e" />
          <rect x="14" y="10" width="7" height="11" rx="1" fill="#e66c37" />
          <rect x="3" y="15" width="10" height="6" rx="1" fill="#6b007b" />
        </svg>
      );
    case "scatter":
    case "bubble":
    case "connected-scatter":
    case "regression":
    case "strip-plot":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <circle cx="6" cy="16" r="2.5" fill="#118dff" />
          <circle cx="11" cy="9" r="3.5" fill="#e66c37" />
          <circle cx="18" cy="6" r="2" fill="#118dff" />
          <circle cx="16" cy="14" r="3" fill="#12239e" />
          <circle cx="8" cy="6" r="1.5" fill="#6b007b" />
        </svg>
      );
    case "radar":
    case "rose":
    case "polar-area":
    case "multi-metric-radar":
      return (
        <svg
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          stroke="#118dff"
          strokeWidth="1.5"
        >
          <polygon points="12 3 20 8 18 18 6 18 4 8" fill="#118dff" fillOpacity="0.25" />
          <line x1="12" y1="12" x2="12" y2="3" />
          <line x1="12" y1="12" x2="20" y2="8" />
          <line x1="12" y1="12" x2="18" y2="18" />
          <line x1="12" y1="12" x2="6" y2="18" />
          <line x1="12" y1="12" x2="4" y2="8" />
        </svg>
      );
    case "radial-gauge":
    case "angular-meter":
    case "half-gauge":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <path
            d="M4 17 A9 9 0 0 1 20 17"
            fill="none"
            stroke="#ededed"
            strokeWidth="3.5"
            strokeLinecap="round"
          />
          <path
            d="M4 17 A9 9 0 0 1 17 8"
            fill="none"
            stroke="#118dff"
            strokeWidth="3.5"
            strokeLinecap="round"
          />
          <circle cx="12" cy="17" r="2" fill="#252423" />
        </svg>
      );
    case "kpi-card":
    case "kpi-sparkline":
      return (
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <text x="3" y="13" fill="#118dff" fontSize="13" fontWeight="bold" fontFamily="sans-serif">
            94%
          </text>
          <polyline
            points="3 19 8 16 13 18 20 14"
            stroke="#107c10"
            strokeWidth="1.8"
            strokeLinecap="round"
          />
        </svg>
      );
    default:
      return (
        <svg
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          stroke="#118dff"
          strokeWidth="2"
        >
          <rect x="4" y="4" width="16" height="16" rx="2" />
          <line x1="8" y1="12" x2="16" y2="12" />
        </svg>
      );
  }
};

export const ChartPickerSheet: React.FC<ChartPickerSheetProps> = ({ isOpen, onClose, onAdd }) => {
  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [selectedDatasetId, setSelectedDatasetId] = useState<number>(1);
  const [selectedCategory, setSelectedCategory] = useState<string>("recommended");
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedChart, setSelectedChart] = useState<ChartCatalogItem | null>(null);

  // Configure Form State
  const [title, setTitle] = useState("");
  const [xField, setXField] = useState("");
  const [yField, setYField] = useState("");
  const [seriesField, setSeriesField] = useState("");

  useEffect(() => {
    if (isOpen) {
      fetchDatasets()
        .then((data) => {
          setDatasets(data);
          if (data.length > 0 && !selectedDatasetId) {
            setSelectedDatasetId(data[0].id);
          }
        })
        .catch(() => {});
    }
  }, [isOpen]);

  // Current dataset & columns
  const currentDataset = datasets.find((d) => d.id === selectedDatasetId) || datasets[0];
  const columns = currentDataset?.columns || [];

  // Auto-prefill fields when chart or dataset changes
  useEffect(() => {
    if (columns.length > 0) {
      if (!xField || !columns.some((c) => c.column_name === xField)) {
        setXField(columns[0]?.column_name || "");
      }
      if (!yField || !columns.some((c) => c.column_name === yField)) {
        setYField(columns[1]?.column_name || columns[0]?.column_name || "");
      }
    }
  }, [selectedDatasetId, columns]);

  // Filtered chart catalog: fast and responsive
  const filteredCharts = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();
    return CHART_CATALOG.filter((item) => {
      const matchesSearch =
        !query ||
        item.name.toLowerCase().includes(query) ||
        item.description.toLowerCase().includes(query) ||
        item.categoryLabel.toLowerCase().includes(query);

      if (query) return matchesSearch;

      if (selectedCategory === "recommended") {
        return item.recommended === true;
      }
      if (selectedCategory === "all") {
        return true;
      }
      return item.category === selectedCategory;
    });
  }, [selectedCategory, searchQuery]);

  /*
   * Escape closes the sheet.
   *
   * The close button has advertised "Close (Esc)" since before the Tailwind
   * migration, and the theme sheet next door already did this -- so rather than
   * delete the promise, honour it.
   */
  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isOpen, onClose]);

  // Handle selecting a chart type to configure
  const handleSelectChart = (chart: ChartCatalogItem) => {
    setSelectedChart(chart);
    setTitle(`${chart.name} - ${currentDataset?.table_name || "Metrics"}`);
  };

  // Handle final submission
  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedChart) return;

    const id = "widget-" + Math.random().toString(36).substring(2, 9);
    const finalTitle = title.trim() || selectedChart.name;

    const widget = selectedChart.buildWidget({
      id,
      title: finalTitle,
      datasetId: selectedDatasetId,
      xField: xField || columns[0]?.column_name || "x",
      yField: yField || columns[1]?.column_name || "y",
      seriesField: seriesField || undefined,
    });

    onAdd(widget);
    onClose();
    setSelectedChart(null);
  };

  if (!isOpen) return null;

  return (
    <div className={BACKDROP} onClick={onClose}>
      <div className={SHEET} onClick={(e) => e.stopPropagation()}>
        {/* Sheet Top Header */}
        <div className={HEADER}>
          <div className={TITLE_GROUP}>
            <h2 className={TITLE}>
              {selectedChart
                ? "Configure Visualization"
                : searchQuery
                  ? `Search Results (${filteredCharts.length})`
                  : selectedCategory === "recommended"
                    ? "Visualizations Gallery (Recommended)"
                    : `Visualizations Gallery (${filteredCharts.length})`}
            </h2>
            <div className={SUBTITLE}>
              {selectedChart
                ? `Customizing ${selectedChart.name} with Power BI Light styling`
                : searchQuery
                  ? `Showing matches across all 63 visuals for "${searchQuery}"`
                  : selectedCategory === "recommended"
                    ? "10 Core visual types • Select category tabs or search to explore all 63"
                    : `Explore ${selectedCategory.toUpperCase()} visuals powered by ECharts & Microsoft Flint`}
            </div>
          </div>
          <div className="flex items-center gap-2">
            <Kbd>Esc</Kbd>
            <Button
              variant="ghost"
              size="icon-sm"
              className={CLOSE}
              onClick={onClose}
              aria-label="Close the visuals gallery"
            >
              ✕
            </Button>
          </div>
        </div>

        {/* STEP 1: BROWSE 63 VISUAL TYPES */}
        {!selectedChart && (
          <div className={CONTENT}>
            {/* Search Box */}
            <div className={SEARCH_BAR}>
              <input
                type="text"
                className={SEARCH_INPUT}
                placeholder="Search across 63 chart types (e.g. donut, gauge, waterfall, area)..."
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                autoFocus
              />
              {searchQuery && (
                <button
                  type="button"
                  aria-label="Clear the search"
                  className={SEARCH_CLEAR}
                  onClick={() => setSearchQuery("")}
                >
                  ✕
                </button>
              )}
            </div>

            {/* Category Filter Pills */}
            <div className={CATEGORY_PILLS} role="group" aria-label="Visual categories">
              {CHART_CATEGORIES.map((cat) => (
                <button
                  key={cat.id}
                  type="button"
                  aria-pressed={selectedCategory === cat.id}
                  className={cn(CATEGORY_PILL, selectedCategory === cat.id && CATEGORY_PILL_ACTIVE)}
                  onClick={() => setSelectedCategory(cat.id)}
                >
                  {cat.label}
                </button>
              ))}
            </div>

            {/* Visuals Grid (63 Types) */}
            <div className={CHART_GRID}>
              {filteredCharts.map((item) => (
                <div
                  key={item.id}
                  role="button"
                  tabIndex={0}
                  aria-label={`Configure the ${item.name} visual`}
                  className={CHART_CARD}
                  onClick={() => handleSelectChart(item)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      handleSelectChart(item);
                    }
                  }}
                >
                  <div className={CHART_CARD_ICON}>
                    <ChartTypeIcon iconType={item.iconType} />
                  </div>
                  <div className={CHART_CARD_INFO}>
                    <span className={CHART_CARD_NAME}>{item.name}</span>
                    <span className={CHART_CARD_CATEGORY}>{item.categoryLabel}</span>
                    <p className={CHART_CARD_DESC}>{item.description}</p>
                  </div>
                </div>
              ))}
              {filteredCharts.length === 0 && (
                <Empty size="compact" className="col-span-2">
                  <EmptyTitle>No visuals found</EmptyTitle>
                  <EmptyDescription>
                    Nothing in the catalogue matches &quot;{searchQuery}&quot;. Try a broader term
                    such as &quot;bar&quot; or &quot;time&quot;.
                  </EmptyDescription>
                </Empty>
              )}
            </div>
          </div>
        )}

        {/* STEP 2: FIELD MAPPING & DATASET CONFIGURATION */}
        {selectedChart && (
          <form className={FORM} onSubmit={handleSubmit}>
            <InlineButton tone="muted" className={BACK_LINK} onClick={() => setSelectedChart(null)}>
              ← Back to all 63 visuals
            </InlineButton>

            {/* Selected Visual Summary Banner */}
            <div className={BANNER}>
              <div className={BANNER_ICON}>
                <ChartTypeIcon iconType={selectedChart.iconType} />
              </div>
              <div className={BANNER_TEXT}>
                <div className={BANNER_TITLE}>{selectedChart.name}</div>
                <div className={BANNER_DESC}>{selectedChart.description}</div>
              </div>
            </div>

            {/* Visual Title */}
            <div className={GROUP}>
              <Label className={LABEL} htmlFor="chart-title">
                Visual Title
              </Label>
              <input
                id="chart-title"
                type="text"
                className={CONTROL}
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="Enter title for visual..."
                required
              />
            </div>

            {/* Superset Dataset Picker */}
            <div className={GROUP}>
              <Label className={LABEL} htmlFor="chart-dataset">
                Superset Query Dataset
              </Label>
              <select
                id="chart-dataset"
                className={CONTROL_SELECT}
                value={selectedDatasetId}
                onChange={(e) => setSelectedDatasetId(Number(e.target.value))}
              >
                {datasets.map((ds) => (
                  <option key={ds.id} value={ds.id}>
                    Dataset #{ds.id}: {ds.table_name} ({ds.database?.database_name || "PostgreSQL"})
                  </option>
                ))}
              </select>
              <span className={HELP}>Live query metadata loaded from Superset REST backend</span>
            </div>

            {/* Primary Field (X-Axis / Category) */}
            <div className={GROUP}>
              <Label className={LABEL} htmlFor="chart-x">
                {selectedChart.fields.xLabel}
              </Label>
              <select
                id="chart-x"
                className={CONTROL_SELECT}
                value={xField}
                onChange={(e) => setXField(e.target.value)}
              >
                {columns.map((col) => (
                  <option key={col.column_name} value={col.column_name}>
                    {col.column_name} ({col.type})
                  </option>
                ))}
              </select>
            </div>

            {/* Secondary Field (Y-Axis / Value / Measure) */}
            <div className={GROUP}>
              <Label className={LABEL} htmlFor="chart-y">
                {selectedChart.fields.yLabel}
              </Label>
              <select
                id="chart-y"
                className={CONTROL_SELECT}
                value={yField}
                onChange={(e) => setYField(e.target.value)}
              >
                {columns.map((col) => (
                  <option key={col.column_name} value={col.column_name}>
                    {col.column_name} ({col.type})
                  </option>
                ))}
              </select>
            </div>

            {/* Optional Legend / Series Breakdown Field */}
            {selectedChart.fields.supportsSeries && (
              <div className={GROUP}>
                <Label className={LABEL} htmlFor="chart-series">
                  {selectedChart.fields.seriesLabel || "Legend / Series (Optional)"}
                </Label>
                <select
                  id="chart-series"
                  className={CONTROL_SELECT}
                  value={seriesField}
                  onChange={(e) => setSeriesField(e.target.value)}
                >
                  <option value="">None (Single Series)</option>
                  {columns.map((col) => (
                    <option key={col.column_name} value={col.column_name}>
                      {col.column_name} ({col.type})
                    </option>
                  ))}
                </select>
              </div>
            )}

            {/* Theme notice */}
            <div className={THEME_TAG}>
              <span className="size-2 rounded-full bg-primary" />
              <span>Theme: Power BI Light (Azure #118dff, Segoe UI, hairline grid)</span>
            </div>

            {/* Action Buttons */}
            <div className={ACTIONS}>
              <Button variant="outline" onClick={() => setSelectedChart(null)}>
                Cancel
              </Button>
              <Button type="submit">Add Visual to Dashboard</Button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
};
