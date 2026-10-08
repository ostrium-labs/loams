/*
 * One icon source.
 *
 * Every icon here is a `lucide-react` component, wrapped only to pin the pixel
 * size the previous hand-drawn SVG used. Keeping the sizes is deliberate: the
 * shell nav, the console rows and the dashboard header were all balanced
 * against those numbers, and resizing them would move every one of those
 * layouts for no benefit.
 *
 * lucide defaults to `strokeWidth={2}`, `currentColor`, round caps/joins and a
 * 24x24 viewBox, which is exactly the convention the hand-rolled icons used --
 * so nothing downstream had to change to accommodate the swap.
 */
import React from "react";
import type { ComponentType } from "react";
import {
  ChartColumn,
  ChartLine,
  ChartPie,
  Calendar,
  Check,
  Ellipsis,
  Info,
  Network,
  Palette,
  Pencil,
  Plus,
  RefreshCw,
  SlidersHorizontal,
  Sparkles,
  Trash,
} from "lucide-react";

type LucideIcon = ComponentType<{ size?: number; className?: string }>;

/**
 * Pin a lucide icon to a fixed square size.
 *
 * `size` is passed through so an individual call site can still override it
 * without reaching past this module.
 */
function sized(Icon: LucideIcon, size: number): LucideIcon {
  const Sized = (props: { size?: number; className?: string }) => (
    <Icon size={props.size ?? size} className={props.className} />
  );
  Sized.displayName = `Sized(${Icon.displayName ?? Icon.name ?? "Icon"})`;
  return Sized;
}

/* Chart type glyphs, used by the chart picker and the widget card header. */
export const LineChartIcon = sized(ChartLine, 15);
export const BarChartIcon = sized(ChartColumn, 15);
export const PieChartIcon = sized(ChartPie, 15);
export const SparklesIcon = sized(Sparkles, 15);
/**
 * The graph widget's header glyph.
 *
 * Sized 15 like its chart siblings so the header icon box holds the same for
 * every tile in the grid.
 */
export const NetworkIcon = sized(Network, 15);

/* Dashboard header controls. */
export const RefreshIcon = sized(RefreshCw, 14);
export const CalendarIcon = sized(Calendar, 13);
export const PlusIcon = sized(Plus, 14);
export const SlidersIcon = sized(SlidersHorizontal, 14);
export const CheckIcon = sized(Check, 14);
export const PaletteIcon = sized(Palette, 14);

/* Inspector affordances. */
export const MoreDotsIcon = sized(Ellipsis, 16);
export const InfoCircleIcon = sized(Info, 13);
export const TrashIcon = sized(Trash, 13);
export const EditIcon = sized(Pencil, 13);
