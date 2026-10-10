/*
 * The shell's icon set.
 *
 * lucide-react is the single source of icon artwork across the app -- these
 * are thin wrappers that pin the same pixel sizes `dashboard-ui`'s
 * `components/Icons.tsx` uses, so the console reads at the same weight as the
 * dashboard header. `ShellMarkIcon` is the one exception and stays hand-drawn:
 * it is the product mark, not a pictogram, and has no lucide equivalent.
 *
 * This module lives in `core/host` rather than importing from
 * `dashboard-ui` because core is the lower layer: dashboard-ui imports
 * `@loams-core/host/ui`, so the dependency cannot also point back.
 */
import React from "react";
import type { ComponentType } from "react";
import { ArrowRight, Check, Gauge, Layers, Plug, TriangleAlert } from "lucide-react";

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

/** A plug, for the plugin list and the toggle panel. */
export const PlugIcon = sized(Plug, 15);

/** A gauge, for the dashboard. */
export const GaugeIcon = sized(Gauge, 15);

/** Stacked panels, for the plugins page. */
export const LayersIcon = sized(Layers, 15);

export const AlertIcon = sized(TriangleAlert, 15);

export const ArrowRightIcon = sized(ArrowRight, 15);

export const CheckIcon = sized(Check, 15);

/**
 * The shell mark: a cordis-style hub with three spokes.
 *
 * Hand-rolled because it is the product's own glyph. Kept on the same 24x24 /
 * `currentColor` / round-cap conventions as lucide so it sits correctly next
 * to the imported icons.
 */
export function ShellMarkIcon() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M12 3v3" />
      <path d="M12 18v3" />
      <path d="M3 12h3" />
      <path d="M18 12h3" />
    </svg>
  );
}
