// The motion system (UI upgrade phase 1; ruling UI1-1 amends D1076). Motion for React is the one
// animation library. Two layers:
//
// - CSS, for Base UI primitives' own enter and exit (`data-starting-style`/`data-ending-style`),
//   on the duration and easing tokens in tokens.css. Base UI waits for those transitions before
//   it unmounts, so focus, dismissal and keyboard behaviour stay Base UI's.
// - Motion (this module), where CSS cannot do it: shared-layout selection indicators, content
//   that mounts and unmounts outside Base UI (skeleton to content, lists), and a custom overlay
//   that wants a Motion `render` element.
//
// Rules: transform and opacity only; nothing animates without a state change; nothing delays
// interaction (an element is interactive while it fades in); prefers-reduced-motion turns every
// preset into an instant change. The numbers match the --op-duration-* and --op-ease-* tokens.

import {
  AnimatePresence,
  type HTMLMotionProps,
  LayoutGroup,
  MotionConfig,
  motion,
  type Transition,
  useReducedMotion,
  type Variants,
} from 'motion/react';
import { type ReactElement, type ReactNode, useId } from 'react';
import { cx } from './cx';

/** Durations in seconds, as Motion takes them; the CSS tokens carry the same values in ms. */
export const MOTION_DURATION = {
  /** Hover and press (--op-duration-fast). */
  fast: 0.12,
  /** Overlays opening (--op-duration-enter). */
  enter: 0.16,
  /** Overlays closing (--op-duration-exit). */
  exit: 0.11,
  /** Pages, toasts and content swaps (--op-duration-page). */
  page: 0.22,
} as const;

/** Cubic-bézier curves; the CSS tokens carry the same. */
export const MOTION_EASE = {
  out: [0.22, 1, 0.36, 1],
  in: [0.4, 0, 1, 1],
  inOut: [0.65, 0, 0.35, 1],
  panel: [0.32, 0.72, 0, 1],
} as const satisfies Record<string, [number, number, number, number]>;

/** Springs, for interactive elements only (a selection that follows the pointer or the keys). */
export const MOTION_SPRING = {
  /** A selection indicator moving between options. */
  indicator: { type: 'spring', stiffness: 520, damping: 42, mass: 0.9 },
  /** A control that answers a press or a drag. */
  snappy: { type: 'spring', stiffness: 640, damping: 40, mass: 0.7 },
} as const satisfies Record<string, Transition>;

/** The instant transition reduced motion gets. */
export const INSTANT: Transition = { duration: 0 };

/** A Motion preset: spread it onto a `motion.*` element. */
export interface MotionPreset {
  initial: HTMLMotionProps<'div'>['initial'];
  animate: HTMLMotionProps<'div'>['animate'];
  exit: HTMLMotionProps<'div'>['exit'];
  transition: Transition;
}

const enter = (duration: number): Transition => ({ duration, ease: MOTION_EASE.out });
const leave = (duration: number): Transition => ({ duration, ease: MOTION_EASE.in });

/** The presets every surface shares. Each one moves transform and opacity only. */
export const MOTION_PRESETS = {
  /** Opacity only: content swaps, toolbars, anything that should not move. */
  fade: {
    initial: { opacity: 0 },
    animate: { opacity: 1 },
    exit: { opacity: 0, transition: leave(MOTION_DURATION.exit) },
    transition: enter(MOTION_DURATION.enter),
  },
  /** A short rise: a page section, a toast, a result arriving. */
  fadeUp: {
    initial: { opacity: 0, y: 6 },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, y: 4, transition: leave(MOTION_DURATION.exit) },
    transition: enter(MOTION_DURATION.page),
  },
  /** Grows from its origin: menus, popovers and dialogs drawn by hand. */
  scaleIn: {
    initial: { opacity: 0, scale: 0.97 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, scale: 0.98, transition: leave(MOTION_DURATION.exit) },
    transition: enter(MOTION_DURATION.enter),
  },
  /** A toast: rises from the bottom edge, leaves faster than it came. */
  toast: {
    initial: { opacity: 0, y: 12 },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, y: 8, transition: leave(MOTION_DURATION.exit) },
    transition: enter(MOTION_DURATION.page),
  },
} as const satisfies Record<string, MotionPreset>;

export type MotionPresetName = keyof typeof MOTION_PRESETS;

/** What a preset becomes under reduced motion: no movement and no fade, an instant change. */
export const REDUCED_PRESET: MotionPreset = {
  initial: false,
  animate: { opacity: 1 },
  exit: { opacity: 0, transition: INSTANT },
  transition: INSTANT,
};

/** A preset, or its reduced form. Pure, for code that already knows the preference. */
export function motionPreset(
  name: MotionPresetName | MotionPreset,
  reduced: boolean,
): MotionPreset {
  if (reduced) return REDUCED_PRESET;
  return typeof name === 'string' ? MOTION_PRESETS[name] : name;
}

/** True when the person asked for reduced motion (`prefers-reduced-motion: reduce`). */
export function usePrefersReducedMotion(): boolean {
  return useReducedMotion() ?? false;
}

/** A preset that follows the person's reduced-motion preference. */
export function useMotionPreset(name: MotionPresetName | MotionPreset): MotionPreset {
  return motionPreset(name, usePrefersReducedMotion());
}

/** A transition that follows the reduced-motion preference: `INSTANT` when reduced. */
export function useMotionTransition(transition: Transition): Transition {
  return usePrefersReducedMotion() ? INSTANT : transition;
}

/**
 * Motion defaults for a tree: reduced motion follows the OS (`reducedMotion="user"` drops
 * transform and layout animations), and the default transition is the enter curve. Wrap an
 * app's root once; the presets and helpers here already respect the preference without it.
 */
export function MotionProvider({ children }: { children: ReactNode }) {
  return (
    <MotionConfig reducedMotion="user" transition={enter(MOTION_DURATION.enter)}>
      {children}
    </MotionConfig>
  );
}

/* Presence -------------------------------------------------------------------------------- */

/**
 * Keeps an overlay mounted while it animates out. For a hand-built Base UI overlay that animates
 * with Motion instead of the primitives' CSS: control `open` on the Root, set `keepMounted` on
 * the Portal, and give the Popup a `render={useMotionRender('scaleIn')}`:
 *
 *   <PopoverRoot open={open} onOpenChange={setOpen}>
 *     <PopoverTrigger>…</PopoverTrigger>
 *     <OverlayPresence open={open}>
 *       <Popover.Portal keepMounted>…<Popover.Popup render={popup}>…
 *
 * Base UI keeps focus, dismissal and the keyboard; Motion only runs the enter and exit.
 */
export function OverlayPresence({
  open,
  children,
  initial = true,
}: {
  open: boolean;
  children: ReactNode;
  /** Animate the first mount when it is already open (false for a restored state). */
  initial?: boolean;
}) {
  return <AnimatePresence initial={initial}>{open ? children : null}</AnimatePresence>;
}

/**
 * A `render` element for a Base UI part (a Popup, a Backdrop) that animates with a preset.
 * Do not combine it with the primitives' own transition classes (OVERLAY_SURFACE carries them).
 */
export function useMotionRender(name: MotionPresetName | MotionPreset = 'scaleIn'): ReactElement {
  const preset = useMotionPreset(name);
  return <motion.div {...preset} />;
}

/* Content swaps --------------------------------------------------------------------------- */

/**
 * Crossfades a skeleton into its content when loading ends. The first render does not
 * animate (a cached page shows its content at once), and the content is interactive from
 * its first frame. The skeleton leaves the layout as it fades, so nothing jumps.
 */
export function LoadingSwap({
  loading,
  skeleton,
  children,
  className,
}: {
  loading: boolean;
  skeleton: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  const preset = useMotionPreset('fade');
  return (
    <div className={cx('relative', className)} data-slot="loading-swap">
      <AnimatePresence initial={false} mode="popLayout">
        {loading ? (
          <motion.div key="skeleton" data-state="loading" {...preset}>
            {skeleton}
          </motion.div>
        ) : (
          <motion.div key="content" data-state="ready" {...preset}>
            {children}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/* Lists ----------------------------------------------------------------------------------- */

/** How much each list item waits after the one before, and after how many items it stops. */
const STAGGER_STEP = 0.025;
const STAGGER_CAP = 8;

/** The list-stagger variants: the container's children rise in order, capped at 8 steps. */
export const LIST_STAGGER: { container: Variants; item: Variants } = {
  container: {
    hidden: {},
    visible: {},
  },
  item: {
    hidden: { opacity: 0, y: 4 },
    visible: (index: number) => ({
      opacity: 1,
      y: 0,
      transition: {
        ...enter(MOTION_DURATION.page),
        delay: Math.min(index, STAGGER_CAP) * STAGGER_STEP,
      },
    }),
    exit: { opacity: 0, transition: leave(MOTION_DURATION.exit) },
  },
};

/**
 * A list whose items rise in order when it first appears (results arriving, a filtered list).
 * Items added later rise on their own. Reduced motion renders it still.
 */
export function StaggerList({
  children,
  className,
  as = 'ul',
  ...props
}: Omit<HTMLMotionProps<'ul'>, 'initial' | 'animate' | 'variants'> & { as?: 'ul' | 'ol' | 'div' }) {
  const reduced = usePrefersReducedMotion();
  const Tag = motion[as] as typeof motion.ul;
  return (
    <Tag
      className={className}
      initial={reduced ? false : 'hidden'}
      animate="visible"
      variants={LIST_STAGGER.container}
      {...props}
    >
      {children}
    </Tag>
  );
}

/** One item of a `StaggerList`; `index` orders the stagger. */
export function StaggerItem({
  index,
  children,
  as = 'li',
  ...props
}: Omit<HTMLMotionProps<'li'>, 'variants' | 'custom'> & { index: number; as?: 'li' | 'div' }) {
  const reduced = usePrefersReducedMotion();
  const Tag = motion[as] as typeof motion.li;
  return (
    <Tag
      custom={index}
      variants={reduced ? undefined : LIST_STAGGER.item}
      transition={reduced ? INSTANT : undefined}
      {...props}
    >
      {children}
    </Tag>
  );
}

/* Selection ------------------------------------------------------------------------------- */

/**
 * Scopes `SelectionIndicator`s, so two tab strips on one page do not share an indicator.
 * Every Tabs and Segmented already wraps its options in one.
 */
export function SelectionGroup({ children }: { children: ReactNode }) {
  const id = useId();
  return <LayoutGroup id={id}>{children}</LayoutGroup>;
}

/**
 * The mark under (or behind) the selected option, which slides to the next selection with a
 * spring. Render it inside the selected option only, with the same `layoutId` in each option:
 *
 *   {selected && <SelectionIndicator layoutId="tab" className="inset-x-2 bottom-0 h-0.5 bg-accent" />}
 *
 * It is decorative (`aria-hidden`); the option's own `aria-selected` or `aria-pressed` says
 * which is selected. Reduced motion moves it instantly.
 */
export function SelectionIndicator({
  layoutId,
  className,
  transition,
  ...props
}: Omit<HTMLMotionProps<'span'>, 'layoutId'> & { layoutId: string }) {
  const reduced = usePrefersReducedMotion();
  return (
    <motion.span
      aria-hidden="true"
      className={cx('pointer-events-none absolute', className)}
      data-slot="selection-indicator"
      layoutId={layoutId}
      transition={reduced ? INSTANT : (transition ?? MOTION_SPRING.indicator)}
      {...props}
    />
  );
}

export { AnimatePresence, motion } from 'motion/react';
