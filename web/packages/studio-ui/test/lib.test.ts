// Behaviour of the generic hooks and utilities vendored as-is in SD1 Task 1 (inventory group k).
import { act, render, renderHook } from '@testing-library/react';
import { createElement } from 'react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { AnimatedHeight, LRUCache, observeResize, useIsMobile, useMediaQuery } from '../src';

describe('LRUCache', () => {
  it('evicts the least recently used entry by count', () => {
    const cache = new LRUCache<string>(2, 1_000);
    cache.set('a', 'A', 1);
    cache.set('b', 'B', 1);
    expect(cache.get('a')).toBe('A');
    cache.set('c', 'C', 1);
    expect(cache.get('b')).toBeNull();
    expect(cache.get('a')).toBe('A');
    expect(cache.get('c')).toBe('C');
  });

  it('evicts by approximate size and refuses entries larger than the budget', () => {
    const cache = new LRUCache<string>(10, 10);
    cache.set('a', 'A', 6);
    cache.set('b', 'B', 6);
    expect(cache.get('a')).toBeNull();
    cache.set('huge', 'H', 11);
    expect(cache.get('huge')).toBeNull();
    expect(cache.get('b')).toBe('B');
  });
});

describe('useMediaQuery', () => {
  const queries: string[] = [];
  afterEach(() => {
    queries.length = 0;
    vi.unstubAllGlobals();
  });
  function stubMatchMedia(matches: boolean) {
    vi.stubGlobal('matchMedia', (query: string) => {
      queries.push(query);
      return {
        matches,
        media: query,
        addEventListener: () => {},
        removeEventListener: () => {},
      };
    });
  }

  it('turns breakpoints into media queries', () => {
    stubMatchMedia(true);
    expect(renderHook(() => useMediaQuery('md:max-xl')).result.current).toBe(true);
    expect(queries).toContain('(min-width: 768px) and (max-width: 1279px)');
    renderHook(() => useMediaQuery({ min: 500, pointer: 'coarse' }));
    expect(queries).toContain('(min-width: 500px) and (pointer: coarse)');
  });

  it('useIsMobile is below the md breakpoint', () => {
    stubMatchMedia(false);
    expect(renderHook(() => useIsMobile()).result.current).toBe(false);
    expect(queries).toContain('(max-width: 767px)');
  });
});

describe('observeResize and AnimatedHeight', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('is a no-op without ResizeObserver', () => {
    vi.stubGlobal('ResizeObserver', undefined);
    const stop = observeResize(document.createElement('div'), () => {});
    expect(() => stop()).not.toThrow();
  });

  it('shares one observer and delivers entries to each subscriber', () => {
    const observed: Element[] = [];
    let deliver: ((entries: ResizeObserverEntry[]) => void) | undefined;
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(cb: (entries: ResizeObserverEntry[]) => void) {
          deliver = cb;
        }
        observe(el: Element) {
          observed.push(el);
        }
        unobserve() {}
        disconnect() {}
      },
    );
    const el = document.createElement('div');
    const seen: number[] = [];
    const stop = observeResize(el, (entries) => seen.push(entries.length));
    expect(observed).toEqual([el]);
    act(() => {
      deliver?.([
        { target: el, contentRect: { width: 1, height: 2 } } as unknown as ResizeObserverEntry,
      ]);
    });
    expect(seen).toEqual([1]);
    stop();
  });

  it('AnimatedHeight renders its children inside the height wrapper', () => {
    const { container } = render(createElement(AnimatedHeight, null, 'hello'));
    const wrapper = container.querySelector('[data-slot="animated-height"]');
    expect(wrapper?.textContent).toBe('hello');
  });
});
