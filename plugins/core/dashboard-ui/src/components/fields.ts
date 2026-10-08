/*
 * Shared form vocabulary for the dashboard's panels.
 *
 * The Inspector, the chart picker, the theme customizer and the per-widget theme
 * field all ask for the same four things: a stacked group, a label, a control,
 * and a line of help text. They used to be `.form-group`, `.form-label`,
 * `.form-input`/`.form-select`/`.sheet-input`/`.sheet-select` and
 * `.sheet-help-text` -- five names for one shape, which is how `.form-label`
 * ended up referenced with no rule at all.
 *
 * Colours come from the `@theme inline` ink tokens, so a Flint theme repaints
 * every panel from the same write that repaints the charts.
 */
import { cn } from "@loams-core/host/ui";

/** A label + control pair, stacked. */
export const GROUP = "flex flex-col gap-[0.35rem]";

/**
 * A text input or a select.
 *
 * Focus is a border change plus the same 2px `--primary-subtle` ring every other
 * control in the app uses, rather than the browser's own outline.
 */
export const CONTROL =
  "w-full rounded border border-line bg-card px-3 py-2 text-[0.82rem] text-ink outline-none transition-[border-color,box-shadow] duration-150 focus:border-primary focus:shadow-[0_0_0_2px_var(--primary-subtle)]";

/** The same, for a select: it needs the pointer affordance the others do not. */
export const CONTROL_SELECT = `${CONTROL} cursor-pointer`;

/** A read-only control, for the Inspector's disabled fields. */
export const CONTROL_DISABLED = "cursor-not-allowed opacity-65";

export const CONTROL_AREA = `${CONTROL} resize-y leading-[1.45]`;

export const CONTROL_MONO =
  "font-[Cascadia_Mono,ui-monospace,SFMono-Regular,Menlo,monospace] text-[0.76rem]";

/** The small grey line under a control. */
export const HELP = "text-[0.72rem] text-ink-muted";

/**
 * The label size.
 *
 * Passed as `className` to the ported `Label`, which ships t3code's own
 * `text-base/4.5` -- right for a settings page, too loud for a 320px sidebar.
 * tailwind-merge resolves the two, font size and weight included.
 */
export const LABEL = "text-[0.78rem] font-semibold text-ink";

/** Invalid-value tint, for a control's own validation. */
export const INVALID =
  "border-danger shadow-[0_0_0_2px_color-mix(in_srgb,var(--danger)_20%,transparent)]";

/** `cn`, re-exported so panels only import one thing from here. */
export { cn };
