import { useCallback, useEffect, useRef, useState } from 'react';

export type Loaded<T> =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; data: T };

export const errorMessage = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/**
 * Runs `load` on mount and whenever `deps` change; `reload` runs it again. A dependency
 * change always shows Loading; a reload does too unless `keepOnReload` keeps the old data
 * in view. `errorText` turns a thrown error into the message shown (default: its message).
 */
export function useLoad<T>(
  load: () => Promise<T>,
  deps: unknown[],
  opts: { keepOnReload?: boolean; errorText?: (e: unknown) => string } = {},
): [Loaded<T>, () => void] {
  const [value, setValue] = useState<Loaded<T>>({ state: 'loading' });
  const [tick, setTick] = useState(0);
  const lastTick = useRef(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  const textRef = useRef(opts.errorText ?? errorMessage);
  textRef.current = opts.errorText ?? errorMessage;
  const keep = opts.keepOnReload === true;
  // biome-ignore lint/correctness/useExhaustiveDependencies: deps are the caller's
  useEffect(() => {
    let live = true;
    if (!keep || lastTick.current === tick) setValue({ state: 'loading' });
    lastTick.current = tick;
    loadRef
      .current()
      .then((data) => live && setValue({ state: 'ready', data }))
      .catch((e) => live && setValue({ state: 'error', message: textRef.current(e) }));
    return () => {
      live = false;
    };
  }, [...deps, tick]);
  return [value, useCallback(() => setTick((t) => t + 1), [])];
}
