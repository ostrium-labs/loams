// A small loader for the billing pages: one call, its result or its error, and a reload. The
// pages read the meter through a client interface, so this stays the same when the fixture
// client is replaced by the Connect one.

import { Notice, plainError, Spinner } from '@loams/ui';
import { type ReactNode, useCallback, useEffect, useState } from 'react';

export type Load<T> =
  | { state: 'loading' }
  | { state: 'ready'; value: T }
  | { state: 'error'; error: unknown };

export function useLoad<T>(
  fetch: () => Promise<T>,
  deps: readonly unknown[] = [],
): [Load<T>, () => void] {
  const [load, setLoad] = useState<Load<T>>({ state: 'loading' });
  const [round, setRound] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the caller names what refetches
  useEffect(() => {
    let live = true;
    fetch().then(
      (value) => live && setLoad({ state: 'ready', value }),
      (error: unknown) => live && setLoad({ state: 'error', error }),
    );
    return () => {
      live = false;
    };
  }, [round, ...deps]);
  const reload = useCallback(() => setRound((r) => r + 1), []);
  return [load, reload];
}

/** The message of whatever a call threw, for a person. */
export function errorText(error: unknown): string {
  return plainError(error instanceof Error ? error.message : String(error));
}

/** Renders `load`: a spinner, the error, or `children` with the value. */
export function Loaded<T>({
  load,
  what,
  children,
}: {
  load: Load<T>;
  what: string;
  children: (value: T) => ReactNode;
}) {
  if (load.state === 'loading') {
    return (
      <div className="py-6">
        <Spinner label={`Loading ${what}`} />
      </div>
    );
  }
  if (load.state === 'error') {
    return (
      <Notice tone="danger" title={`Could not load ${what}`}>
        {errorText(load.error)}
      </Notice>
    );
  }
  return <>{children(load.value)}</>;
}

/** A fresh idempotency key for one mutation (AP0: every mutation carries one). */
export function idempotencyKey(): string {
  return (
    globalThis.crypto?.randomUUID?.() ?? `k-${Date.now()}-${Math.random().toString(36).slice(2)}`
  );
}
