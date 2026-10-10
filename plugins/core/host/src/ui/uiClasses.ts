/*
 * Shared Tailwind class fragments for the shell's pages.
 *
 * The console, the plugins page and the plugin page all render the same page
 * furniture -- a centred column, an empty/not-found panel, a couple of button
 * shapes. Before the Tailwind migration those lived as `.page`, `.empty-state`
 * and `.btn*` rules in `styles.css`, which every one of those files had to
 * remember to apply correctly.
 *
 * They are plain strings rather than components because most of them wrap
 * content that differs per page. The T3 primitives in
 * `dashboard-ui/src/components/ui/` cover the genuinely reusable *controls*;
 * this file is only the layout vocabulary those pages share.
 *
 * Colour comes from the `@theme inline` tokens, which are `--th-*` under the
 * hood -- so a chart-specs theme repaints all of this with no extra wiring.
 */

/** The centred content column every shell page sits in. */
export const PAGE = "flex w-full max-w-[1120px] flex-1 flex-col gap-7 px-6 pt-7 pb-12 mx-auto";

export const PAGE_HEAD = "flex flex-wrap items-start justify-between gap-6";

export const PAGE_HEAD_TEXT = "flex min-w-0 flex-col gap-[0.3rem]";

export const PAGE_TITLE = "text-xl leading-[1.25] font-semibold tracking-[-0.015em] text-ink";

export const PAGE_SUBTITLE = "max-w-[62ch] text-[0.82rem] leading-6 text-ink-body";

export const PAGE_HEAD_ACTIONS = "flex shrink-0 items-center gap-[0.6rem]";

export const PAGE_COUNT =
  "whitespace-nowrap text-[0.78rem] text-ink-body [&_strong]:font-semibold [&_strong]:text-ink";

/* ------------------------------------------------------- empty states --- */

/**
 * The frame the three "nothing to show" panels share.
 *
 * `Empty` supplies the centring and the stack; these are the dashboard's own
 * card chrome, so the panels still read as a card against the page rather than
 * as bare centred text.
 */
export const EMPTY_STATE =
  "flex flex-col items-center gap-[0.55rem] rounded-lg border border-line bg-card px-6 py-12 text-center shadow-card";

export const EMPTY_STATE_MARK =
  "flex size-11 items-center justify-center rounded-lg border border-line-subtle bg-page text-ink-muted";

export const EMPTY_STATE_TITLE = "text-base font-semibold tracking-[-0.01em] text-ink";

export const EMPTY_STATE_BODY = "max-w-[52ch] text-[0.81rem] leading-[1.55] text-ink-body";

export const EMPTY_STATE_ACTIONS = "mt-[0.35rem] flex flex-wrap justify-center gap-[0.6rem]";

export const EMPTY_STATE_PATH =
  "rounded-[3px] border border-primary-border bg-primary-subtle px-[0.35rem] py-[0.1rem] font-mono text-[0.78rem] text-primary-ink";

/*
 * The loading placeholder.
 *
 * The ported `Skeleton` supplies the shimmer and the rounding; these are only
 * the dimensions, because a page's loading state has to hold its shape so
 * nothing jumps when the data lands.
 */
export const SKELETON_LINE = "h-[9px] w-full max-w-[34rem]";

export const SKELETON_TITLE = "h-[13px] w-64 max-w-full";

export const SKELETON_TAIL = "h-[9px] w-80 max-w-full";

/* ----------------------------------------------------------- buttons --- */

export const BTN_BASE =
  "inline-flex cursor-pointer items-center justify-center gap-[0.4rem] rounded border border-transparent px-[0.9rem] py-[0.45rem] font-sans text-[0.82rem] font-medium transition-all duration-150";

export const BTN_PRIMARY = "border-primary-solid bg-primary-solid text-white hover:brightness-95";

export const BTN_OUTLINE =
  "border-line bg-card text-ink-body hover:border-ink-subtle hover:bg-page hover:text-ink";

export const BTN_SM = "px-2.5 py-1 text-[0.74rem]";
