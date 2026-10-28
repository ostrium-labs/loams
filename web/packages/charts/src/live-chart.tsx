import { useEffect, useMemo, useRef, useState } from 'react';
import { ChartSpecError, compileSpec, xFieldOf } from './compile';
import { DataTable } from './data-table';
import { usePrefersReducedMotion } from './motion';
import { useChartTheme } from './theme';
import { createThrottle } from './throttle';
import type { ChartSpec, Row } from './types';
import { useECharts } from './use-echarts';
import { appendWindow, windowBounds } from './window';

/** Shortest gap between two repaints of a streaming chart. */
export const REPAINT_INTERVAL_MS = 250;

export interface LiveChartProps {
  spec: ChartSpec;
  /** Initial (or static) rows. A new array resets the chart to it. */
  data: readonly Record<string, unknown>[];
  /** Batches appended as they arrive. A new iterable restarts the subscription. */
  stream?: AsyncIterable<Record<string, unknown>[]>;
  /** Keep only rows within this many ms of the newest one (time = the spec's x field). */
  windowMs?: number;
  height?: number;
  /** What the chart shows, for assistive tech. Required: a chart without one is a blank image. */
  ariaLabel: string;
}

/**
 * A chart from a ChartSpec, optionally fed by a stream.
 *
 * Rows live in a ref and repaint through a throttle, so a fast stream costs one
 * `setOption` per {@link REPAINT_INTERVAL_MS}, not one per batch. The canvas is
 * hidden from assistive tech; the figure carries `ariaLabel` and a visually
 * hidden table of the latest rows stands in for it.
 */
export function LiveChart({
  spec,
  data,
  stream,
  windowMs,
  height = 240,
  ariaLabel,
}: LiveChartProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const chart = useECharts(hostRef);
  const theme = useChartTheme();
  const reducedMotion = usePrefersReducedMotion();

  // Inline props are new objects on every render; key the reset on content so a
  // parent that does not memoise them does not reset the chart in a loop.
  const specKey = JSON.stringify(spec);
  const stableData = useShallowStable(data);
  const timeField = xFieldOf(spec);
  const win = useMemo(
    () =>
      windowMs !== undefined && windowMs > 0 && timeField !== undefined
        ? { windowMs, timeField }
        : undefined,
    [windowMs, timeField],
  );

  const rowsRef = useRef<Row[]>([]);
  const seriesOrderRef = useRef<string[]>([]);
  const [shown, setShown] = useState<readonly Row[]>([]);
  const [error, setError] = useState<string | undefined>();
  const [streamError, setStreamError] = useState<string | undefined>();

  // The latest paint function, read by the long-lived throttle.
  const paintRef = useRef<() => void>(() => {});
  paintRef.current = () => {
    const rows = rowsRef.current;
    setShown(rows);
    const instance = chart.current;
    try {
      const compiled = compileSpec(spec, rows, {
        theme,
        animate: !reducedMotion,
        seriesOrder: seriesOrderRef.current,
        window: win ? windowBounds(rows, win.timeField, win.windowMs) : undefined,
      });
      seriesOrderRef.current = compiled.seriesOrder;
      setError(undefined);
      instance?.setOption(compiled.option, { notMerge: true, lazyUpdate: true });
    } catch (err) {
      setError(err instanceof ChartSpecError ? err.message : 'This chart could not be drawn.');
      instance?.clear();
    }
  };

  const throttle = useMemo(() => createThrottle(() => paintRef.current(), REPAINT_INTERVAL_MS), []);
  useEffect(() => () => throttle.cancel(), [throttle]);

  // New data, window or spec: reset and paint at once.
  // oxlint-disable-next-line react/exhaustive-deps -- specKey is the trigger that stands in for spec; paintRef reads the current spec.
  useEffect(() => {
    seriesOrderRef.current = [];
    rowsRef.current = win ? appendWindow([], stableData as Row[], win) : [...(stableData as Row[])];
    throttle.cancel();
    paintRef.current();
  }, [stableData, win, specKey, throttle]);

  // Theme or motion preference changed: repaint the same rows.
  // oxlint-disable-next-line react/exhaustive-deps -- theme and reducedMotion are triggers; paintRef reads their current values.
  useEffect(() => {
    paintRef.current();
  }, [theme, reducedMotion]);

  // Streaming append.
  useEffect(() => {
    if (!stream) return;
    setStreamError(undefined);
    const iterator = stream[Symbol.asyncIterator]();
    let stopped = false;
    void (async () => {
      try {
        while (!stopped) {
          const next = await iterator.next();
          if (stopped || next.done) break;
          const batch = next.value as Row[];
          // Without a window the row cap still applies, so an endless stream stays bounded.
          rowsRef.current = appendWindow(
            rowsRef.current,
            batch,
            win ?? { windowMs: 0, timeField: timeField ?? '' },
          );
          throttle.schedule();
        }
      } catch (err) {
        if (!stopped) setStreamError(err instanceof Error ? err.message : String(err));
      }
    })();
    return () => {
      stopped = true;
      void iterator.return?.();
    };
  }, [stream, win, timeField, throttle]);

  return (
    <figure style={{ margin: 0, position: 'relative' }} aria-label={ariaLabel}>
      <div
        ref={hostRef}
        aria-hidden="true"
        data-testid="loams-chart-canvas"
        style={{ width: '100%', height, display: error ? 'none' : undefined }}
      />
      {error && (
        <p
          role="status"
          style={{
            margin: 0,
            minHeight: height,
            display: 'grid',
            placeItems: 'center',
            color: 'var(--op-muted)',
            fontSize: 13,
            textAlign: 'center',
          }}
        >
          {error}
        </p>
      )}
      {streamError && (
        <p role="status" style={{ margin: '4px 0 0', color: 'var(--op-danger)', fontSize: 12 }}>
          Live updates stopped: {streamError}
        </p>
      )}
      <DataTable rows={shown} caption={`${ariaLabel}: data`} timeField={timeField} />
    </figure>
  );
}

/** The previous array when `next` holds the same elements, so `data={[]}` is stable. */
function useShallowStable<T>(next: readonly T[]): readonly T[] {
  const ref = useRef(next);
  const prev = ref.current;
  if (prev !== next && (prev.length !== next.length || prev.some((item, i) => item !== next[i]))) {
    ref.current = next;
  }
  return ref.current;
}
