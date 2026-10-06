/**
 * The chrome every widget tile is built from.
 *
 * Extracted from `WidgetCard.tsx` unchanged, because two tiles rendering the
 * dashboard have to be visually identical -- same radius, same padding, same
 * title size, same hover behaviour, same header height. Duplicating a dozen
 * Tailwind class strings into `GraphWidget` would make them drift silently: one
 * tile would end up a pixel taller than its neighbour and nothing would fail.
 *
 * `CardFrame` is therefore the single source for the card box, the header, the
 * title/subtitle pair, the hover-revealed action row, and the body well that the
 * loading and error overlays sit inside. The two concrete cards supply only what
 * is genuinely theirs: an icon, a title, and the contents of the body.
 */
import React from "react";
import { cn } from "@loams-plugins/core/ui";
import { EditIcon, TrashIcon } from "./Icons";

/*
 * The visual tile.
 *
 * `group` on the root is what reveals the action row on hover; the
 * `.widget-card:hover .widget-actions.hover-visible` selector and the
 * `group-hover:` utility express the same rule.
 */
export const CARD =
  "group flex h-full flex-col overflow-hidden rounded border border-line bg-card shadow-card";

export const CARD_HEADER =
  "flex items-start justify-between gap-3 border-b border-line-subtle bg-card px-[1.15rem] pt-[0.95rem] pb-[0.65rem]";

export const TITLE_AREA = "flex min-w-0 flex-1 flex-col gap-[0.2rem]";

export const CARD_ICON =
  "inline-flex size-[26px] shrink-0 items-center justify-center rounded bg-primary-subtle text-primary [&_svg]:size-4";

export const CARD_TITLE =
  "m-0 truncate font-['-apple-system','BlinkMacSystemFont','Segoe_UI',Roboto,sans-serif] text-[1.125rem] leading-[1.25] font-bold tracking-[-0.015em] text-ink";

/** Indented to sit under the title, level with the icon's baseline. */
export const SUBTITLE_ROW = "flex items-center gap-[0.35rem] pl-[calc(26px+0.55rem)]";

export const CARD_SUBTITLE = "truncate text-[0.8rem] leading-[1.3] font-medium text-ink-body";

export const ACTION_BTN =
  "inline-flex cursor-pointer items-center gap-[0.25rem] rounded-[3px] border border-line bg-transparent px-[0.45rem] py-[0.2rem] text-[0.72rem] font-medium text-ink-body transition-all duration-150 hover:border-ink-muted hover:bg-page hover:text-ink";

/* The two washes are written out rather than interpolated: Tailwind scans
   source text for complete class names, so `bg-[${tint}]` would compile to
   nothing. See the same note in `ConsolePage`. */
export const ACTION_BTN_DANGER =
  "border-[color-mix(in_srgb,var(--danger)_28%,var(--bg-card))] text-danger hover:border-danger hover:bg-[color-mix(in_srgb,var(--danger)_6%,var(--bg-card))]";

export const CARD_BODY = "relative flex w-full flex-1 flex-col min-h-40 bg-plot";

/*
 * The canvas is painted from the theme's plot ink, and this overlay sits on top
 * of it while the body is still loading.
 */
export const STATE_OVERLAY =
  "absolute inset-0 z-5 flex flex-col items-center justify-center gap-3 bg-[color-mix(in_srgb,var(--bg-card)_90%,transparent)]";

export const SPINNER =
  "size-6 rounded-full border-[2.5px] border-line-subtle border-t-primary animate-spin-slow";

interface CardFrameProps {
  widgetId: string;
  title: string;
  subtitle?: string;
  icon: React.ReactNode;
  editMode: boolean;
  isSelected: boolean;
  onSelect: (widgetId: string) => void;
  onDelete: (widgetId: string) => void;
  /** The canvas: a chart, a graph, a table. */
  children: React.ReactNode;
}

/**
 * One widget tile: box, header, actions, and a body well.
 *
 * The header is identical across every tile so the dashboard grid reads as one
 * system. `onSelect` fires on a click anywhere on the card, but only in edit
 * mode -- in read mode a click on a chart must reach the chart, not select it.
 */
export const CardFrame: React.FC<CardFrameProps> = ({
  widgetId,
  title,
  subtitle,
  icon,
  editMode,
  isSelected,
  onSelect,
  onDelete,
  children,
}) => (
  <div
    className={cn(
      CARD,
      "transition-[border-color,box-shadow] duration-150",
      "hover:border-line-subtle hover:shadow-hover",
      isSelected && "border-primary shadow-[0_0_0_2px_var(--primary-subtle)]",
    )}
    onClick={() => editMode && onSelect(widgetId)}
  >
    <div className={CARD_HEADER}>
      <div className={TITLE_AREA}>
        <div className="flex items-center gap-[0.55rem]">
          <span className={CARD_ICON}>{icon}</span>
          <h3 className={CARD_TITLE} title={title}>
            {title}
          </h3>
        </div>
        {subtitle && (
          <div className={SUBTITLE_ROW}>
            <span className={CARD_SUBTITLE} title={subtitle}>
              {subtitle}
            </span>
          </div>
        )}
      </div>

      {/*
       * Hover-revealed in read mode, always shown in edit mode -- `group` on
       * the card above is what reveals it, and the two variants differ only in
       * whether they start at zero.
       */}
      <div
        className={cn(
          "flex shrink-0 items-center gap-[0.35rem]",
          editMode
            ? "opacity-100"
            : "opacity-0 transition-opacity duration-150 group-hover:opacity-100",
        )}
      >
        <button
          type="button"
          className={ACTION_BTN}
          title="Edit Widget"
          onClick={(e) => {
            e.stopPropagation();
            onSelect(widgetId);
          }}
        >
          <EditIcon />
          <span>Edit</span>
        </button>
        <button
          type="button"
          className={cn(ACTION_BTN, ACTION_BTN_DANGER)}
          title="Delete Widget"
          onClick={(e) => {
            e.stopPropagation();
            onDelete(widgetId);
          }}
        >
          <TrashIcon />
          <span>Remove</span>
        </button>
      </div>
    </div>

    <div className={CARD_BODY}>{children}</div>
  </div>
);
