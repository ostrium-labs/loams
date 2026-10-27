// The motion system (UI1): presets, the CSS duration tokens they mirror, presence for Base UI
// overlays, the skeleton-to-content swap and the sliding selection.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Popover } from '@base-ui/react/popover';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MotionGlobalConfig } from 'motion/react';
import { useState } from 'react';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vite-plus/test';
import {
  LoadingSwap,
  MOTION_DURATION,
  MOTION_PRESETS,
  motionPreset,
  OverlayPresence,
  REDUCED_PRESET,
  Segmented,
  Tabs,
  useMotionRender,
} from '../src';

// jsdom replaces URL, so the path is built from the test's directory.
const TOKENS = readFileSync(join(import.meta.dirname, '../src/tokens.css'), 'utf8');

// jsdom has no layout or animation frames worth waiting for: run every animation to its end.
beforeAll(() => {
  MotionGlobalConfig.skipAnimations = true;
});
afterAll(() => {
  MotionGlobalConfig.skipAnimations = false;
});
afterEach(cleanup);

describe('presets', () => {
  it('move transform and opacity only', () => {
    const allowed = new Set(['opacity', 'x', 'y', 'scale', 'transition']);
    for (const [name, preset] of Object.entries(MOTION_PRESETS)) {
      for (const state of [preset.initial, preset.animate, preset.exit]) {
        for (const key of Object.keys(state as object)) {
          expect(allowed.has(key), `${name} animates ${key}`).toBe(true);
        }
      }
    }
  });

  it('leave faster than they arrive', () => {
    for (const preset of Object.values(MOTION_PRESETS)) {
      const exit = (preset.exit as { transition: { duration: number } }).transition.duration;
      expect(exit).toBeLessThan(preset.transition.duration as number);
    }
  });

  it('stay within the owner’s ranges: hover 120–180ms, pages 180–280ms', () => {
    expect(MOTION_DURATION.fast).toBeGreaterThanOrEqual(0.12);
    expect(MOTION_DURATION.fast).toBeLessThanOrEqual(0.18);
    expect(MOTION_DURATION.page).toBeGreaterThanOrEqual(0.18);
    expect(MOTION_DURATION.page).toBeLessThanOrEqual(0.28);
  });

  it('become an instant change under reduced motion', () => {
    for (const name of Object.keys(MOTION_PRESETS) as (keyof typeof MOTION_PRESETS)[]) {
      expect(motionPreset(name, true)).toBe(REDUCED_PRESET);
      expect(motionPreset(name, false)).toBe(MOTION_PRESETS[name]);
    }
    expect(REDUCED_PRESET.initial).toBe(false);
    expect(REDUCED_PRESET.transition).toEqual({ duration: 0 });
  });
});

describe('the CSS tokens', () => {
  const block = (from: string) =>
    TOKENS.slice(TOKENS.indexOf(from), TOKENS.indexOf('\n}', TOKENS.indexOf(from)));
  const ms = (name: string) =>
    Number(new RegExp(`${name}:\\s*(\\d+)ms`).exec(block(':root {'))?.[1]);

  it('carry the same durations as the presets', () => {
    expect(ms('--op-duration-fast')).toBe(MOTION_DURATION.fast * 1000);
    expect(ms('--op-duration-enter')).toBe(MOTION_DURATION.enter * 1000);
    expect(ms('--op-duration-exit')).toBe(MOTION_DURATION.exit * 1000);
    expect(ms('--op-duration-page')).toBe(MOTION_DURATION.page * 1000);
  });

  it('are all zero under prefers-reduced-motion', () => {
    const declared = [...block(':root {').matchAll(/(--op-[a-z-]*duration[a-z-]*):/g)].map(
      (m) => m[1],
    );
    expect(declared.length).toBeGreaterThanOrEqual(6);
    const reduce = TOKENS.slice(TOKENS.indexOf('@media (prefers-reduced-motion: reduce)'));
    const zeroed = reduce.slice(0, reduce.indexOf('\n}'));
    for (const name of declared) expect(zeroed, name).toContain(`${name}: 0ms;`);
  });
});

/** A hand-built Base UI popover that animates with Motion (the README's recipe). */
function MotionPopover() {
  const [open, setOpen] = useState(false);
  const popup = useMotionRender('scaleIn');
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger>Filters</Popover.Trigger>
      <OverlayPresence open={open}>
        <Popover.Portal keepMounted>
          <Popover.Positioner>
            <Popover.Popup render={popup}>
              <Popover.Title>Filters</Popover.Title>
              <button type="button">Apply</button>
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </OverlayPresence>
    </Popover.Root>
  );
}

describe('OverlayPresence with a Base UI popover', () => {
  it('mounts on open, keeps Base UI’s dialog semantics, and unmounts after closing', async () => {
    render(<MotionPopover />);
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Filters' }));
    const dialog = await screen.findByRole('dialog');
    expect(dialog.getAttribute('aria-labelledby')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Filters' }).getAttribute('aria-expanded')).toBe(
      'true',
    );
    fireEvent.keyDown(dialog, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(screen.getByRole('button', { name: 'Filters' }).getAttribute('aria-expanded')).toBe(
      'false',
    );
  });
});

describe('LoadingSwap', () => {
  it('shows the skeleton, then the content, without animating the first render', async () => {
    const { rerender } = render(
      <LoadingSwap loading skeleton={<p>Loading rows</p>}>
        <p>Rows</p>
      </LoadingSwap>,
    );
    expect(screen.getByText('Loading rows')).toBeTruthy();
    // The first render is not an entrance: nothing starts transparent.
    expect(screen.getByText('Loading rows').parentElement?.style.opacity).not.toBe('0');
    rerender(
      <LoadingSwap loading={false} skeleton={<p>Loading rows</p>}>
        <p>Rows</p>
      </LoadingSwap>,
    );
    expect(screen.getByText('Rows')).toBeTruthy();
    await waitFor(() => expect(screen.queryByText('Loading rows')).toBeNull());
  });

  it('renders content at once when it is not loading', () => {
    render(
      <LoadingSwap loading={false} skeleton={<p>Loading rows</p>}>
        <button type="button">Open</button>
      </LoadingSwap>,
    );
    expect(screen.queryByText('Loading rows')).toBeNull();
    expect(screen.getByRole('button', { name: 'Open' })).toBeTruthy();
  });
});

describe('the sliding selection', () => {
  function TwoTabs() {
    const [value, setValue] = useState<'a' | 'b'>('a');
    return (
      <Tabs
        label="Views"
        value={value}
        onValueChange={setValue}
        tabs={[
          { value: 'a', label: 'Rows', panel: <p>row panel</p> },
          { value: 'b', label: 'Schema', panel: <p>schema panel</p> },
        ]}
      />
    );
  }

  it('sits in the selected tab only, hidden from assistive technology', async () => {
    render(<TwoTabs />);
    const rows = screen.getByRole('tab', { name: 'Rows' });
    const schema = screen.getByRole('tab', { name: 'Schema' });
    const indicator = () => document.querySelectorAll('[data-slot="selection-indicator"]');
    expect(indicator()).toHaveLength(1);
    expect(rows.contains(indicator()[0] as Node)).toBe(true);
    expect(indicator()[0]?.getAttribute('aria-hidden')).toBe('true');
    await act(async () => fireEvent.click(schema));
    expect(schema.getAttribute('aria-selected')).toBe('true');
    expect(indicator()).toHaveLength(1);
    expect(schema.contains(indicator()[0] as Node)).toBe(true);
    expect(screen.getByText('schema panel')).toBeTruthy();
  });

  it('keeps two strips on one page apart', () => {
    render(
      <>
        <TwoTabs />
        <TwoTabs />
      </>,
    );
    expect(document.querySelectorAll('[data-slot="selection-indicator"]')).toHaveLength(2);
  });

  it('follows the pressed segment, and the group keeps aria-pressed', async () => {
    function Mode() {
      const [mode, setMode] = useState<'console' | 'studio'>('console');
      return (
        <Segmented
          label="Mode"
          value={mode}
          onValueChange={setMode}
          options={[
            { value: 'console', label: 'Console' },
            { value: 'studio', label: 'Studio' },
          ]}
        />
      );
    }
    render(<Mode />);
    const studio = screen.getByRole('button', { name: 'Studio' });
    await act(async () => fireEvent.click(studio));
    expect(studio.getAttribute('aria-pressed')).toBe('true');
    expect(studio.querySelector('[data-slot="selection-indicator"]')).not.toBeNull();
  });
});
