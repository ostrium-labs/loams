// Panel motion (UI1-1, UI2): the sidebar, the inspector and the drawer snap open and shut
// unless Settings › Appearance › Motion is on. The CSS token `--op-panel-duration` already
// says which: 0ms by default and under `prefers-reduced-motion`, 300ms with Motion on. Reading
// it at the moment a panel toggles keeps the one rule in tokens.css.

import { INSTANT, MOTION_EASE } from '@loams/ui';
import { useState } from 'react';

type Transition = typeof INSTANT;

/** The panel duration in seconds right now: 0 unless Motion is on and motion is not reduced. */
export function panelSeconds(root: Element | null = globalThis.document?.documentElement): number {
  if (!root) return 0;
  try {
    const raw = getComputedStyle(root).getPropertyValue('--op-panel-duration').trim();
    const n = Number.parseFloat(raw);
    if (!Number.isFinite(n) || n <= 0) return 0;
    return raw.endsWith('ms') ? n / 1000 : n;
  } catch {
    return 0;
  }
}

export interface PanelToggle {
  /** Render the panel's content: open, or still closing. */
  shown: boolean;
  /** Closing with motion: the content is on its way out (make it inert). */
  closing: boolean;
  /** A toggle's size animation is running (or about to): hold the content at its size. */
  animating: boolean;
  /** The transition for the panel's size: the panel curve on a toggle, instant otherwise. */
  transition: Transition;
  /** Call when the size animation completes. */
  done: () => void;
}

/**
 * Tracks a panel's open state for a size animation. A toggle gets the panel duration (so a
 * drag, which changes the size every frame, stays instant), and a panel that closes with
 * motion stays rendered until `done`. With Motion off nothing waits: `shown` is `open`.
 */
export function usePanelToggle(open: boolean): PanelToggle {
  const [prev, setPrev] = useState(open);
  const [seconds, setSeconds] = useState(0);
  const [closing, setClosing] = useState(false);
  if (prev !== open) {
    const s = panelSeconds();
    setPrev(open);
    setSeconds(s);
    setClosing(!open && s > 0);
  }
  return {
    shown: open || closing,
    closing,
    animating: seconds > 0,
    transition: seconds > 0 ? { duration: seconds, ease: MOTION_EASE.panel } : INSTANT,
    done: () => {
      setSeconds(0);
      setClosing(false);
    },
  };
}
