import { act, cleanup, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import type { ChartSpec } from '../src/types';

// ECharts needs a real canvas; record what the component asks of it instead.
const echartsMock = vi.hoisted(() => {
  const instances: {
    setOption: ReturnType<typeof vi.fn>;
    dispose: ReturnType<typeof vi.fn>;
    resize: ReturnType<typeof vi.fn>;
    clear: ReturnType<typeof vi.fn>;
    isDisposed: () => boolean;
  }[] = [];
  return {
    instances,
    init: vi.fn(() => {
      const inst = {
        setOption: vi.fn(),
        dispose: vi.fn(),
        resize: vi.fn(),
        clear: vi.fn(),
        isDisposed: () => false,
      };
      instances.push(inst);
      return inst;
    }),
  };
});
vi.mock('echarts', () => ({ init: echartsMock.init }));

const { LiveChart, REPAINT_INTERVAL_MS } = await import('../src/live-chart');

/** A stream the test pushes batches into by hand. */
function manualStream() {
  const queue: Record<string, unknown>[][] = [];
  let wake: (() => void) | undefined;
  let closed = false;
  const returned = vi.fn();
  const iterable: AsyncIterable<Record<string, unknown>[]> = {
    [Symbol.asyncIterator]() {
      return {
        async next() {
          while (queue.length === 0 && !closed) await new Promise<void>((r) => (wake = r));
          const value = queue.shift();
          return value ? { done: false, value } : { done: true, value: undefined };
        },
        async return() {
          closed = true;
          returned();
          wake?.();
          return { done: true, value: undefined };
        },
      };
    },
  };
  return {
    iterable,
    returned,
    push(batch: Record<string, unknown>[]) {
      queue.push(batch);
      wake?.();
    },
  };
}

const spec: ChartSpec = {
  chartType: 'Line Chart',
  encodings: { x: { field: 't', type: 'temporal' }, y: 'v' },
};

beforeEach(() => {
  echartsMock.instances.length = 0;
  echartsMock.init.mockClear();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const lastOption = (): Record<string, unknown> => {
  const option = echartsMock.instances.at(-1)?.setOption.mock.calls.at(-1)?.[0];
  if (option === undefined) throw new Error('setOption was never called');
  return option as Record<string, unknown>;
};

describe('LiveChart', () => {
  it('labels the figure, hides the canvas and renders the data table', () => {
    render(
      <LiveChart
        spec={spec}
        data={[
          { t: 1_000, v: 1 },
          { t: 2_000, v: 2 },
        ]}
        ariaLabel="Requests per second"
      />,
    );
    const figure = screen.getByRole('figure', { name: 'Requests per second' });
    expect(within(figure).getByTestId('loams-chart-canvas').getAttribute('aria-hidden')).toBe(
      'true',
    );
    const table = within(figure).getByRole('table', { name: /Requests per second: data/ });
    expect(within(table).getAllByRole('row')).toHaveLength(3);
    expect(within(table).getByText('1970-01-01T00:00:02.000Z')).toBeTruthy();
  });

  it('inits ECharts once, sets a compiled option and disposes on unmount', () => {
    const { unmount } = render(<LiveChart spec={spec} data={[{ t: 1, v: 1 }]} ariaLabel="x" />);
    expect(echartsMock.init).toHaveBeenCalledTimes(1);
    const option = lastOption();
    expect((option.series as { type: string }[])[0]?.type).toBe('line');
    const inst = echartsMock.instances[0];
    unmount();
    expect(inst?.dispose).toHaveBeenCalled();
  });

  it('appends streamed batches inside the window, throttling repaints', async () => {
    const stream = manualStream();
    render(
      <LiveChart
        spec={spec}
        data={[{ t: 0, v: 1 }]}
        stream={stream.iterable}
        windowMs={10_000}
        ariaLabel="Live"
      />,
    );
    await act(async () => {
      stream.push([{ t: 5_000, v: 2 }]);
      await new Promise((r) => setTimeout(r, 0));
    });
    await act(async () => {
      stream.push([{ t: 15_000, v: 3 }]);
      await new Promise((r) => setTimeout(r, REPAINT_INTERVAL_MS + 20));
    });
    const option = lastOption();
    expect((option.dataset as { source: unknown[] }).source).toEqual([
      { t: 5_000, v: 2 },
      { t: 15_000, v: 3 },
    ]);
    expect((option.xAxis as Record<string, unknown>[])[0]).toMatchObject({
      min: 5_000,
      max: 15_000,
    });
    expect(screen.getAllByRole('row')).toHaveLength(3);
  });

  it('closes the stream on unmount', async () => {
    const stream = manualStream();
    const { unmount } = render(
      <LiveChart spec={spec} data={[]} stream={stream.iterable} ariaLabel="Live" />,
    );
    unmount();
    await act(async () => {});
    expect(stream.returned).toHaveBeenCalled();
  });

  it('shows a message and keeps the table for a spec it cannot draw', () => {
    render(
      <LiveChart
        spec={{ chartType: 'Network Graph', encodings: {} }}
        data={[{ a: 1 }]}
        ariaLabel="Graph"
      />,
    );
    expect(screen.getByRole('status').textContent).toMatch(/@loams-core\/flows/);
    expect(screen.getByRole('table', { name: /Graph: data/ })).toBeTruthy();
  });

  it('does not reset in a loop when props are inline literals', () => {
    const { rerender } = render(<LiveChart spec={{ ...spec }} data={[]} ariaLabel="x" />);
    const calls = echartsMock.instances[0]?.setOption.mock.calls.length ?? 0;
    rerender(<LiveChart spec={{ ...spec }} data={[]} ariaLabel="x" />);
    expect(echartsMock.instances[0]?.setOption.mock.calls.length).toBe(calls);
  });

  it('turns animation off under prefers-reduced-motion', () => {
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({
      matches: query.includes('reduce'),
      media: query,
      addEventListener() {},
      removeEventListener() {},
    })) as unknown as typeof window.matchMedia;
    try {
      render(<LiveChart spec={spec} data={[{ t: 1, v: 1 }]} ariaLabel="x" />);
      expect(lastOption().animation).toBe(false);
    } finally {
      window.matchMedia = original;
    }
  });
});
