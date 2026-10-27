// What every Engines page shares: the honest Preview badge (D2019), the sample-data notice,
// a phase tag, and a small loader hook.

import { PageHead } from '@loams/desktop-ui';
import { Badge, Notice, SampleDataNotice, type Status, StatusTag } from '@loams/ui';
import { type ReactNode, useCallback, useEffect, useState } from 'react';
import {
  type EnginesClient,
  errorText,
  type HealthState,
  type Instance,
  isEntitlementExhausted,
  KIND_LABELS,
  type Phase,
} from './client.js';

export const ENGINES_PACKAGE = '@loams/plugin-engines';

/** Every managed engine is preview until design 81 says otherwise (D2019). */
export function PreviewBadge() {
  return (
    <Badge variant="warning" title="Managed engines are a preview: APIs and sizes may change.">
      Preview
    </Badge>
  );
}

export function EnginesHead({
  client,
  title,
  subtitle,
  actions,
  crumbs,
}: {
  client: EnginesClient;
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  crumbs?: ReactNode;
}) {
  return (
    <>
      <PageHead
        title={title}
        badge={<PreviewBadge />}
        subtitle={subtitle}
        actions={actions}
        crumbs={crumbs}
      />
      {client.sample && (
        <SampleDataNotice>
          This server does not serve the managed-engines API yet; changes here touch only the
          sample.
        </SampleDataNotice>
      )}
    </>
  );
}

const PHASE_STATUS: Record<Phase, Status> = {
  pending: 'planned',
  provisioning: 'progress',
  running: 'done',
  scaling: 'progress',
  upgrading: 'progress',
  suspending: 'progress',
  suspended: 'neutral',
  resuming: 'progress',
  deleting: 'progress',
  deleted: 'neutral',
  failed: 'failed',
};

export function PhaseTag({ phase }: { phase: Phase }) {
  return <StatusTag status={PHASE_STATUS[phase]}>{phase}</StatusTag>;
}

const HEALTH_VARIANT = {
  healthy: 'success',
  degraded: 'warning',
  unhealthy: 'danger',
  unknown: 'neutral',
} as const;

export function HealthBadge({ health }: { health: HealthState }) {
  return <Badge variant={HEALTH_VARIANT[health]}>{health}</Badge>;
}

export function KindLabel({ kind }: { kind: Instance['kind'] }) {
  return <span data-kind={kind}>{KIND_LABELS[kind]}</span>;
}

export type Loaded<T> =
  | { state: 'loading' }
  | { state: 'ok'; value: T }
  | { state: 'error'; message: string };

/** Loads `load()` when its deps change; `reload` runs it again. */
export function useLoad<T>(
  load: () => Promise<T>,
  deps: readonly unknown[],
): [Loaded<T>, () => void] {
  const [out, setOut] = useState<Loaded<T>>({ state: 'loading' });
  const [tick, setTick] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the caller names the deps
  useEffect(() => {
    let live = true;
    load().then(
      (value) => live && setOut({ state: 'ok', value }),
      (e) => live && setOut({ state: 'error', message: errorText(e) }),
    );
    return () => {
      live = false;
    };
  }, [...deps, tick]);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  return [out, reload];
}

/**
 * A failed action. A 402 `entitlement_exhausted` (Loams Meter, design 63) is said plainly:
 * the plan's allowance is used up, nothing changed, and what to do about it.
 */
export function ActionError({ error, title }: { error: unknown; title: string }) {
  if (isEntitlementExhausted(error)) {
    return (
      <Notice tone="warn" title="Out of allowance">
        {error.message}. Nothing was changed: add credit or raise the spend cap in Settings,
        Billing, or wait for the allowance to reset, then try again.
      </Notice>
    );
  }
  return (
    <Notice tone="danger" title={title}>
      {errorText(error)}
    </Notice>
  );
}

export function LoadError({ message }: { message: string }) {
  return (
    <Notice tone="danger" title="Could not load">
      {message}
    </Notice>
  );
}

/** The detail route of an instance. */
export const instanceHref = (i: Pick<Instance, 'namespace' | 'id'>, rest = '') =>
  `/engines/${encodeURIComponent(i.namespace)}/${encodeURIComponent(i.id)}${rest}`;
