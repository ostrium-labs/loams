// The `/postgres` route's pg-control client: probed once, kept while it answers, and
// dropped when App settings > Postgres saves a new URL or key file, so the page
// follows the change without a restart.
import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { useEffect, useState } from 'react';
import { resolvePgControl } from './control/connect.js';
import { createMockPgControl } from './control/mock.js';
import type { PgControlClient } from './control/types.js';
import { PostgresPage } from './page.js';

export interface PgControlSource {
  /** The client: pg-control's when it answers, else the fixture-backed mock. */
  resolve(): Promise<PgControlClient>;
  /** Calls `cb` after a saved settings change; returns its unsubscribe. */
  onChange(cb: () => void): () => void;
  /** Stops following the desktop's settings. */
  dispose(): void;
}

const hostOf = (url: string): string | undefined => {
  try {
    return new URL(url).host;
  } catch {
    return undefined;
  }
};

export function createPgControlSource(
  desktop: Pick<LoamsDesktopApi, 'pgControl'>,
  baseUrl: string,
  fetchImpl?: typeof globalThis.fetch,
): PgControlSource {
  let control: Promise<PgControlClient> | undefined;
  const listeners = new Set<() => void>();
  const off = desktop.pgControl?.onChanged(() => {
    control = undefined;
    for (const l of listeners) l();
  });
  const probe = async (): Promise<PgControlClient> => {
    // Where requests go, for the page's subtitle; main holds the key and proxies.
    const where = await desktop.pgControl?.settings().catch(() => undefined);
    return resolvePgControl(
      {
        baseUrl,
        ...(fetchImpl ? { fetch: fetchImpl } : {}),
        ...(where ? { endpoint: hostOf(where.effectiveUrl) } : {}),
      },
      () => createMockPgControl(),
    );
  };
  const resolve = (): Promise<PgControlClient> => {
    if (control) return control;
    const next: Promise<PgControlClient> = probe().then((client) => {
      // Probe again on the next visit when pg-control was not up yet.
      if (client.mock && control === next) control = undefined;
      return client;
    });
    control = next;
    return next;
  };
  return {
    resolve,
    onChange(cb) {
      listeners.add(cb);
      return () => void listeners.delete(cb);
    },
    dispose() {
      off?.();
      listeners.clear();
    },
  };
}

export function PostgresRoute({
  desktop,
  source,
}: {
  desktop: Parameters<typeof PostgresPage>[0]['desktop'];
  source: PgControlSource;
}) {
  const [control, setControl] = useState<PgControlClient>();
  useEffect(() => {
    let live = true;
    let latest = 0;
    // After a saved change the previous client stays on screen until the new one answers;
    // an older probe that answers late is dropped.
    const load = () => {
      const mine = ++latest;
      void source.resolve().then((c) => live && mine === latest && setControl(c));
    };
    load();
    const off = source.onChange(load);
    return () => {
      live = false;
      off();
    };
  }, [source]);
  return <PostgresPage desktop={desktop} control={control} />;
}
