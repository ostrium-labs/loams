// prefers-reduced-motion: reduce (UI1). Its own file: Motion reads the preference once per
// page, so the media query is stubbed before anything renders.

import { cleanup, render, renderHook, screen } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it } from 'vite-plus/test';
import {
  LoadingSwap,
  REDUCED_PRESET,
  StaggerItem,
  StaggerList,
  useMotionPreset,
  useMotionTransition,
  usePrefersReducedMotion,
} from '../src';

beforeAll(() => {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: (query: string) => ({
      // Motion asks for "(prefers-reduced-motion)"; CSS asks for ": reduce".
      matches: query.includes('prefers-reduced-motion') && !query.includes('no-preference'),
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }),
  });
});
afterEach(cleanup);

describe('reduced motion', () => {
  it('is read from the OS', () => {
    const { result } = renderHook(() => usePrefersReducedMotion());
    expect(result.current).toBe(true);
  });

  it('turns every preset into an instant change', () => {
    for (const name of ['fade', 'fadeUp', 'scaleIn', 'toast'] as const) {
      const { result } = renderHook(() => useMotionPreset(name));
      expect(result.current).toBe(REDUCED_PRESET);
    }
  });

  it('turns a spring into an instant transition', () => {
    const { result } = renderHook(() =>
      useMotionTransition({ type: 'spring', stiffness: 500, damping: 40 }),
    );
    expect(result.current).toEqual({ duration: 0 });
  });

  it('renders a stagger list still: no item starts hidden or offset', () => {
    render(
      <StaggerList aria-label="Results">
        {['a', 'b', 'c'].map((id, index) => (
          <StaggerItem key={id} index={index}>
            {id}
          </StaggerItem>
        ))}
      </StaggerList>,
    );
    for (const item of screen.getAllByRole('listitem')) {
      expect(item.style.opacity).not.toBe('0');
      expect(item.style.transform ?? '').not.toMatch(/translate/);
    }
  });

  it('swaps a skeleton for content at once', () => {
    const { rerender } = render(
      <LoadingSwap loading skeleton={<p>Loading</p>}>
        <p>Ready</p>
      </LoadingSwap>,
    );
    rerender(
      <LoadingSwap loading={false} skeleton={<p>Loading</p>}>
        <p>Ready</p>
      </LoadingSwap>,
    );
    expect(screen.getByText('Ready').parentElement?.style.opacity).not.toBe('0');
  });
});
