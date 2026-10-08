import React, { useEffect, useRef, useState } from "react";
import * as echarts from "echarts";
import { Widget, previewWidget } from "../api";
import { composeChartOption, paramFilterFor } from "./chartOption";
import { LineChartIcon, BarChartIcon, PieChartIcon, SparklesIcon } from "./Icons";
import { CardFrame, SPINNER, STATE_OVERLAY } from "./cardChrome";

/*
 * The card chrome -- box, header, title, action row, body well -- is shared with
 * `GraphWidget` through `cardChrome.tsx`. It was moved out verbatim when the
 * graph tile arrived, so that two tiles in the same grid cannot drift apart by a
 * pixel. Nothing about the chart's own behaviour lives there; the ECharts mount
 * and the option composition below are still this component's alone.
 */

interface WidgetCardProps {
  widget: Widget;
  editMode: boolean;
  isSelected: boolean;
  params?: Record<string, unknown>;
  onSelect: (widget: Widget) => void;
  onDelete: (widgetId: string) => void;
  onParamChange?: (name: string, value: unknown) => void;
  /**
   * True when a theme applies to this widget.
   *
   * The Power BI Light chrome below is the dashboard's own default styling. When
   * a theme is in play the server maps ThemeInk onto the ECharts option, and
   * overwriting its axis, grid, legend and tooltip colours here would undo
   * that mapping — so the colour-bearing blocks stand down. Structural tidying
   * (title suppression, grid padding, label formatters) stays either way: that
   * is chrome layout, not ink.
   */
  hasTheme?: boolean;
  /**
   * The dashboard-level theme selection, sent with the preview request so the
   * compiled option matches what the dashboard will actually render. A widget
   * with its own `chartSpec.theme_spec` overrides this server-side.
   */
  dashboardTheme?: unknown;
}

export const WidgetCard: React.FC<WidgetCardProps> = ({
  widget,
  editMode,
  isSelected,
  params,
  onSelect,
  onDelete,
  onParamChange,
  hasTheme = false,
  dashboardTheme,
}) => {
  const chartRef = useRef<HTMLDivElement>(null);
  const instanceRef = useRef<echarts.ECharts | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  /**
   * A `loams` tile shows current state, so it re-reads; a BI backend tile is a
   * warehouse scan over a period and would only get more expensive.
   */
  const live = widget.data.source === "loams";
  const LIVE_REFRESH_MS = 5_000;

  // Load preview data & compile option. Re-runs on an interval for live sources,
  // so a dropped watch stream that recovers shows up without a reload.
  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);

    let timer: ReturnType<typeof setInterval> | undefined;
    const load = () =>
      previewWidget(widget, params, dashboardTheme)
        .then((res) => {
          if (!active) return;
          setLoading(false);
          // A successful re-read clears a previous failure, so a stream that
          // recovers does not leave a stale error on screen.
          setError(null);
          applyPreview(res);
        })
        .catch((err) => {
          if (!active) return;
          setLoading(false);
          setError(err.message || "Failed to render chart");
        });

    load();
    if (live) timer = setInterval(load, LIVE_REFRESH_MS);

    return () => {
      active = false;
      if (timer) clearInterval(timer);
    };
  }, [widget, JSON.stringify(params), hasTheme, live]);

  const applyPreview = (res: { option: Record<string, unknown> }) => {
    if (!chartRef.current) return;
    if (!instanceRef.current) {
      instanceRef.current = echarts.init(chartRef.current);
    }

    const option = composeChartOption(res.option as Record<string, unknown>, hasTheme);
    instanceRef.current.setOption(option, true);

    // Attach interaction listener for cross-widget filtering
    instanceRef.current.off("click");
    instanceRef.current.on("click", (clicked: echarts.ECElementEvent) => {
      if (!onParamChange) return;
      const filter = paramFilterFor(widget, params);
      for (const [paramName, value] of Object.entries(filter)) {
        onParamChange(paramName, value);
      }
    });
  };

  // Handle Resize
  useEffect(() => {
    if (!chartRef.current) return;
    const resizeObserver = new ResizeObserver(() => {
      instanceRef.current?.resize();
    });
    resizeObserver.observe(chartRef.current);

    return () => {
      resizeObserver.disconnect();
      if (instanceRef.current) {
        instanceRef.current.dispose();
        instanceRef.current = null;
      }
    };
  }, []);

  const overrides = (widget.chart?.optionOverrides as any) || {};
  const title =
    overrides?.title?.text ||
    widget.chartSpec?.title ||
    (widget.chart?.kind ? `${widget.chart.kind.toUpperCase()} Chart` : "Chart Widget");

  const subtitle =
    overrides?.title?.subtext ||
    widget.chartSpec?.subtitle ||
    (widget.data?.datasetId ? `Dataset #${widget.data.datasetId}` : "");

  const renderChartIcon = () => {
    if (widget.chartSpec) return <SparklesIcon />;
    const kind = widget.chart?.kind;
    if (kind === "bar") return <BarChartIcon />;
    if (kind === "pie") return <PieChartIcon />;
    return <LineChartIcon />;
  };

  return (
    <CardFrame
      widgetId={widget.id}
      title={title}
      subtitle={subtitle}
      icon={renderChartIcon()}
      editMode={editMode}
      isSelected={isSelected}
      onSelect={() => onSelect(widget)}
      onDelete={onDelete}
    >
      {loading && (
        <div className={STATE_OVERLAY}>
          <div className={SPINNER} />
        </div>
      )}
      {error && (
        <div className={STATE_OVERLAY}>
          <span className="text-[0.85rem] text-danger">{error}</span>
        </div>
      )}

      <div
        ref={chartRef}
        className="h-full w-full flex-1"
        style={{ opacity: loading || error ? 0 : 1 }}
      />
    </CardFrame>
  );
};
