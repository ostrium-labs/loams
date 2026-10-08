import React, { useState, useEffect, useMemo } from "react";
import { Responsive, WidthProvider, Layout } from "react-grid-layout/legacy";
import {
  DashboardSpec,
  Widget,
  fetchDashboard,
  patchDashboard,
  addWidgetToDashboard,
  removeWidgetFromDashboard,
} from "./api";
import { WidgetCard } from "./components/WidgetCard";
import { GraphWidget } from "./components/GraphWidget";
import { Inspector } from "./components/Inspector";
import { ChartPickerSheet } from "./components/ChartPickerSheet";
import { ThemePickerSheet } from "./components/ThemePickerSheet";
import { PaletteIcon } from "./components/Icons";
import { useTheme } from "./theme/useTheme";
import type { ThemeSelection } from "./theme/types";
import { RefreshIcon, CalendarIcon, PlusIcon, SlidersIcon, CheckIcon } from "./components/Icons";
import { Badge, Button, cn } from "@loams-core/host/ui";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";

const ResponsiveGridLayout = WidthProvider(Responsive);

const DEFAULT_DASHBOARD_ID = "e3b0c442-98fc-1c14-9afbf4c8996fb924";

/* ------------------------------------------------------------- fragments */

/*
 * The header, the filter bar and the two full-page states.
 *
 * Written as utilities rather than the `.dashboard-*` rules this file used to
 * lean on. The colours are the `--th-*` ink tokens through the `@theme inline`
 * aliases (`bg-card`, `text-ink-body`, ...), so a Flint theme repaints the whole
 * header from the same write that repaints the charts.
 */

const SPINNER =
  "size-6 rounded-full border-[2.5px] border-line-subtle border-t-primary animate-spin-slow";

/** Full-page state: a centred spinner or a failure with a way out. */
const STATE_PAGE =
  "absolute inset-0 z-5 flex flex-col items-center justify-center gap-3 bg-[color-mix(in_srgb,var(--bg-card)_90%,transparent)]";

const HEADER =
  "flex flex-wrap items-center justify-between gap-5 border-b border-line bg-card px-8 pt-4 pb-[0.85rem]";

const HEADER_LEFT = "flex flex-col gap-1";

const TITLE_ROW = "flex items-center gap-3";

const TITLE = "text-[1.3rem] font-semibold tracking-[-0.01em] text-ink";

const DESCRIPTION = "text-[0.82rem] text-ink-body";

const HEADER_RIGHT = "flex items-center gap-[0.65rem]";

const SEGMENTED = "inline-flex items-center rounded border border-line bg-page p-0.5";

const SEGMENTED_BTN =
  "inline-flex cursor-pointer items-center gap-[0.3rem] rounded-[3px] border-none bg-transparent px-[0.65rem] py-[0.3rem] font-sans text-[0.78rem] font-medium text-ink-body transition-all duration-150 hover:text-ink";

const SEGMENTED_BTN_ACTIVE = "bg-card font-semibold text-ink shadow-[0_1px_2px_rgba(0,0,0,0.08)]";

/** The last segment is visually separated from the range list. */
const SEGMENTED_CUSTOM = "border-l border-line pl-[0.55rem]";

const TOGGLE_ACTIVE = "border-success/40 bg-success/8 font-semibold text-success";

const FILTER_BAR =
  "flex items-center justify-between gap-4 border-b border-line bg-card px-8 py-[0.55rem]";

const FILTER_LEFT = "flex flex-wrap items-center gap-[0.6rem]";

const FILTER_LABEL = "text-[0.78rem] font-semibold text-ink-body";

const FILTER_PILL =
  "inline-flex items-center gap-[0.35rem] rounded-[3px] border border-primary-border bg-primary-subtle px-[0.55rem] py-[0.15rem] text-[0.75rem] text-primary-ink";

const FILTER_PILL_REMOVE =
  "cursor-pointer border-none bg-none text-[0.95rem] leading-none text-primary transition-colors hover:text-primary-ink";

const FILTER_CLEAR =
  "cursor-pointer border-none bg-none text-[0.75rem] text-ink-muted underline hover:text-ink";

const CONTENT = "relative flex flex-1 px-8 pt-6 pb-12";

export function App() {
  const [dashboardId] = useState<string>(() => {
    const urlParams = new URLSearchParams(window.location.search);
    return urlParams.get("id") || DEFAULT_DASHBOARD_ID;
  });

  const [spec, setSpec] = useState<DashboardSpec | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [timeRange, setTimeRange] = useState("30d");
  const [activeParamFilters, setActiveParamFilters] = useState<Record<string, unknown>>({});
  const [editMode, setEditMode] = useState(false);
  const [selectedWidget, setSelectedWidget] = useState<Widget | null>(null);
  const [isSheetOpen, setIsSheetOpen] = useState(false);
  const [isThemeSheetOpen, setIsThemeSheetOpen] = useState(false);

  // Themes are fetched independently of the dashboard and can fail on their
  // own without affecting the page: the hook degrades to "no catalogue" rather
  // than rejecting, and nothing below awaits it.
  const theme = useTheme({ initialSelection: spec?.theme ?? null });

  const loadDashboard = () => {
    setLoading(true);
    fetchDashboard(dashboardId)
      .then((data) => {
        setSpec(data);
        setLoading(false);
      })
      .catch((err) => {
        setError(err.message);
        setLoading(false);
      });
  };

  useEffect(() => {
    loadDashboard();
  }, [dashboardId]);

  const handleParamChange = (name: string, value: unknown) => {
    setActiveParamFilters((prev) => ({
      ...prev,
      [name]: value,
    }));
  };

  const currentParams = useMemo(
    () => ({
      time_range: timeRange,
      ...activeParamFilters,
    }),
    [timeRange, activeParamFilters],
  );

  const handleLayoutChange = async (currentLayout: Layout) => {
    if (!spec || !editMode) return;

    const ops: any[] = [];
    currentLayout.forEach((item) => {
      const idx = spec.layout.findIndex((l) => l.id === item.i);
      if (idx !== -1) {
        const prev = spec.layout[idx];
        if (prev.x !== item.x || prev.y !== item.y || prev.w !== item.w || prev.h !== item.h) {
          ops.push({ op: "replace", path: `/layout/${idx}/x`, value: item.x });
          ops.push({ op: "replace", path: `/layout/${idx}/y`, value: item.y });
          ops.push({ op: "replace", path: `/layout/${idx}/w`, value: item.w });
          ops.push({ op: "replace", path: `/layout/${idx}/h`, value: item.h });
        }
      }
    });

    if (ops.length > 0) {
      try {
        const updated = await patchDashboard(spec.id, spec.version, ops);
        setSpec(updated);
      } catch {
        // A failed layout save is not worth interrupting the drag for; the
        // next interaction re-patches. The cards stay where they were dropped.
      }
    }
  };

  const handleAddWidget = async (newWidget: Widget) => {
    if (!spec) return;
    try {
      const maxY = spec.layout.reduce((max, item) => Math.max(max, item.y + item.h), 0);
      const position = { x: 0, y: maxY, w: 6, h: 4 };
      const updated = await addWidgetToDashboard(spec.id, newWidget, position);
      setSpec(updated);
    } catch (err: any) {
      alert("Failed to add chart: " + err.message);
    }
  };

  const handleDeleteWidget = async (widgetId: string) => {
    if (!spec) return;
    try {
      // Optimistic instant removal from UI state
      const updatedWidgets = { ...spec.widgets };
      delete updatedWidgets[widgetId];
      const updatedLayout = spec.layout.filter((l) => l.id !== widgetId);
      setSpec({ ...spec, widgets: updatedWidgets, layout: updatedLayout });
      if (selectedWidget?.id === widgetId) setSelectedWidget(null);

      const updated = await removeWidgetFromDashboard(spec.id, widgetId);
      setSpec(updated);
    } catch {
      // Re-read from the server so the optimistic removal is undone rather
      // than leaving the grid showing a chart that no longer exists.
      loadDashboard();
    }
  };

  const handleUpdateWidget = async (updatedWidget: Widget) => {
    if (!spec) return;
    try {
      const ops = [{ op: "replace", path: `/widgets/${updatedWidget.id}`, value: updatedWidget }];
      const updated = await patchDashboard(spec.id, spec.version, ops);
      setSpec(updated);
      setSelectedWidget(updatedWidget);
      alert("Chart updated successfully!");
    } catch (err: any) {
      alert("Failed to update chart: " + err.message);
    }
  };

  /**
   * Persist the dashboard-level house.
   *
   * Stored at `/theme` on the spec. A widget's own `flint.theme_spec` still
   * wins over this — that precedence is the server's, and the copy in both the
   * picker and the Inspector says so.
   *
   * `add` rather than `replace` because the field is usually absent, and
   * `remove` when the choice is "no theme" so the spec carries no empty value.
   */
  const handleSaveTheme = async (selection: ThemeSelection | null) => {
    if (!spec) return;

    // A JSON Patch `remove` against a path that is not there throws, so "no
    // theme" on a dashboard that never had one is a no-op rather than a failed
    // save. There is nothing to write and nothing to undo.
    if (selection === null && (spec.theme === null || spec.theme === undefined)) {
      setIsThemeSheetOpen(false);
      return;
    }

    const op =
      selection === null
        ? { op: "remove", path: "/theme" }
        : { op: "add", path: "/theme", value: selection };
    try {
      const updated = await patchDashboard(spec.id, spec.version, [op]);
      setSpec(updated);
      setIsThemeSheetOpen(false);
    } catch (err: any) {
      alert("Failed to save theme: " + err.message);
    }
  };

  if (loading) {
    return (
      <div className={cn(STATE_PAGE)} style={{ height: "100vh" }}>
        <div className={SPINNER} />
        <p>Loading Dashboard...</p>
      </div>
    );
  }

  if (error || !spec) {
    return (
      <div className={cn(STATE_PAGE)} style={{ height: "100vh" }}>
        <h2>Failed to Load Dashboard</h2>
        <p className="text-danger">{error || "Dashboard not found"}</p>
        <Button onClick={loadDashboard}>Retry</Button>
      </div>
    );
  }

  // `Layout` is `readonly LayoutItem[]` in react-grid-layout 2.x.
  const gridLayout: Layout = spec.layout.map((item) => ({
    i: item.id,
    x: item.x,
    y: item.y,
    w: item.w,
    h: item.h,
    minW: 3,
    minH: 3,
  }));

  const filterKeys = Object.keys(activeParamFilters);

  return (
    <div className="flex min-h-screen flex-col">
      {/* 1. Clean Chart Dashboard Header */}
      <header className={HEADER}>
        <div className={HEADER_LEFT}>
          <div className={TITLE_ROW}>
            <h1 className={TITLE}>{spec.title || "Analytics & Performance"}</h1>
            <Badge variant="success" size="sm" className="gap-[0.35rem]">
              <span className="size-1.5 animate-live-pulse rounded-full bg-current" />
              Live Backend
            </Badge>
          </div>
          <div className={DESCRIPTION}>
            Interactive ECharts visualizations powered by Apache Superset and Microsoft Flint
          </div>
        </div>

        <div className={HEADER_RIGHT}>
          {/* Time range segmented control */}
          <div className={SEGMENTED}>
            {["7d", "30d", "60d", "90d"].map((range) => (
              <button
                key={range}
                type="button"
                aria-pressed={timeRange === range}
                className={cn(SEGMENTED_BTN, timeRange === range && SEGMENTED_BTN_ACTIVE)}
                onClick={() => setTimeRange(range)}
              >
                {range}
              </button>
            ))}
            <button
              type="button"
              aria-pressed={timeRange.startsWith("custom")}
              className={cn(
                SEGMENTED_BTN,
                SEGMENTED_CUSTOM,
                timeRange.startsWith("custom") && SEGMENTED_BTN_ACTIVE,
              )}
              title="Custom date range"
              onClick={() => {
                const customVal = prompt(
                  "Enter custom date range (e.g. 2025-01-01:2025-01-05):",
                  "2025-01-01:2025-01-05",
                );
                if (customVal) setTimeRange(`custom:${customVal}`);
              }}
            >
              <CalendarIcon />
              <span>{timeRange.startsWith("custom") ? "Custom *" : "Custom"}</span>
            </button>
          </div>

          <Button
            variant="outline"
            size="sm"
            className={cn(isThemeSheetOpen && TOGGLE_ACTIVE)}
            onClick={() => setIsThemeSheetOpen(true)}
            title="Choose the dashboard theme"
          >
            <PaletteIcon />
            <span>Theme</span>
          </Button>

          <Button variant="outline" size="icon-sm" title="Refresh data" onClick={loadDashboard}>
            <RefreshIcon />
          </Button>

          <Button size="sm" onClick={() => setIsSheetOpen(true)}>
            <PlusIcon />
            <span>Add Visual</span>
          </Button>

          {/* Edit / Customize layout toggle */}
          <Button
            variant="outline"
            size="sm"
            className={cn(editMode && TOGGLE_ACTIVE)}
            onClick={() => {
              setEditMode(!editMode);
              if (editMode) setSelectedWidget(null);
            }}
          >
            {editMode ? (
              <>
                <CheckIcon />
                <span>Done</span>
              </>
            ) : (
              <>
                <SlidersIcon />
                <span>Customize</span>
              </>
            )}
          </Button>
        </div>
      </header>

      {/* 2. Active Interactive Filter Bar (if user clicked a chart segment) */}
      {filterKeys.length > 0 && (
        <div className={FILTER_BAR}>
          <div className={FILTER_LEFT}>
            <span className={FILTER_LABEL}>Cross-chart filter:</span>
            {filterKeys.map((key) => (
              <span key={key} className={FILTER_PILL}>
                <strong>{key}:</strong> {String(activeParamFilters[key])}
                <button
                  type="button"
                  aria-label={`Remove the ${key} filter`}
                  className={FILTER_PILL_REMOVE}
                  onClick={() => {
                    const next = { ...activeParamFilters };
                    delete next[key];
                    setActiveParamFilters(next);
                  }}
                >
                  &times;
                </button>
              </span>
            ))}
            <button
              type="button"
              className={FILTER_CLEAR}
              onClick={() => setActiveParamFilters({})}
            >
              Reset filters
            </button>
          </div>
        </div>
      )}

      {/* 3. Pure Charts Grid */}
      <main className={CONTENT}>
        <div className="w-full flex-1">
          <ResponsiveGridLayout
            layouts={{ lg: gridLayout }}
            breakpoints={{ lg: 1200, md: 996, sm: 768, xs: 480, xxs: 0 }}
            cols={{ lg: 12, md: 12, sm: 6, xs: 4, xxs: 2 }}
            rowHeight={98}
            isDraggable={editMode}
            isResizable={editMode}
            onDragStop={handleLayoutChange}
            onResizeStop={handleLayoutChange}
            margin={[18, 18]}
          >
            {spec.layout.map((item) => {
              const widget = spec.widgets[item.id];
              if (!widget) return null;

              return (
                <div key={item.id}>
                  {/*
                   * `graph` is a sibling widget type, not a chart kind, so it
                   * gets its own tile and its own preview route rather than a
                   * branch inside `WidgetCard` -- React Flow owns its canvas,
                   * its drag/zoom/pan state and its teardown, and none of that
                   * is an ECharts concern.
                   *
                   * A graph widget cannot carry `flint.theme_spec` (the schema
                   * forbids it), so the dashboard theme is unconditionally the
                   * effective one for it -- no per-widget override to prefer.
                   */}
                  {widget.type === "graph" ? (
                    <GraphWidget
                      widget={widget}
                      editMode={editMode}
                      isSelected={selectedWidget?.id === widget.id}
                      params={currentParams}
                      dashboardTheme={spec.theme}
                      onSelect={setSelectedWidget}
                      onDelete={handleDeleteWidget}
                      onParamChange={handleParamChange}
                    />
                  ) : (
                    <WidgetCard
                      widget={widget}
                      editMode={editMode}
                      isSelected={selectedWidget?.id === widget.id}
                      params={currentParams}
                      /*
                       * Per-widget override wins over the dashboard theme, so
                       * this is the effective one for that widget. It tells the
                       * card whether the server owns its colours — see the
                       * `hasTheme` note in WidgetCard.
                       */
                      hasTheme={Boolean(widget.flint?.theme_spec ?? spec.theme)}
                      dashboardTheme={spec.theme}
                      onSelect={setSelectedWidget}
                      onDelete={handleDeleteWidget}
                      onParamChange={handleParamChange}
                    />
                  )}
                </div>
              );
            })}
          </ResponsiveGridLayout>
        </div>

        {/* Inspector Sidebar when in Edit Mode */}
        {editMode && selectedWidget && (
          <Inspector
            widget={selectedWidget}
            onClose={() => setSelectedWidget(null)}
            onUpdate={handleUpdateWidget}
            onDelete={handleDeleteWidget}
            themePresets={theme.presets}
            dashboardTheme={spec.theme ?? null}
            themeCatalogError={theme.catalogError}
          />
        )}
      </main>

      {/* Theme picker + customizer */}
      <ThemePickerSheet
        isOpen={isThemeSheetOpen}
        onClose={() => setIsThemeSheetOpen(false)}
        savedSelection={spec.theme ?? null}
        theme={theme}
        onSave={handleSaveTheme}
      />

      {/* 63 Visuals Chart Picker Sheet */}
      <ChartPickerSheet
        isOpen={isSheetOpen}
        onClose={() => setIsSheetOpen(false)}
        onAdd={handleAddWidget}
      />
    </div>
  );
}

export default App;
