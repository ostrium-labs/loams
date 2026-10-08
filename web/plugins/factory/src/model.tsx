// Shared pieces: app roles, health labels, the query hook and the panel card.

import type {
  FactoryAppId,
  FactoryAppInfo,
  FactoryHealth,
  LoamsDesktopApi,
} from '@loams/desktop/contracts';
import { Button, Card, Notice, type Status, StatusTag } from '@loams/ui';
import { RefreshCw } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';

/** One line on what each app is for (§37 §19.2). */
export const ROLES: Record<FactoryAppId, string> = {
  forgejo: 'Repos, PRs, CI',
  zulip: 'Team chat and channels',
  plane: 'Issues and project planning',
  glitchtip: 'Error tracking',
  openpanel: 'Product analytics',
  matomo: 'Web analytics',
  langfuse: 'LLM traces and cost',
  openobserve: 'Logs, metrics and traces',
};

export const APP_LABELS: Record<FactoryAppId, string> = {
  forgejo: 'Forgejo',
  zulip: 'Zulip',
  plane: 'Plane (ItsAPlan)',
  glitchtip: 'GlitchTip',
  openpanel: 'OpenPanel',
  matomo: 'Matomo',
  langfuse: 'Langfuse',
  openobserve: 'OpenObserve',
};

export const HEALTH: Record<FactoryHealth, { status: Status; label: string }> = {
  ok: { status: 'done', label: 'Connected' },
  auth_failed: { status: 'failed', label: 'Auth failed' },
  unreachable: { status: 'failed', label: 'Unreachable' },
  unconfigured: { status: 'neutral', label: 'Not configured' },
};

export function HealthPill({ health }: { health: FactoryHealth }) {
  return <StatusTag status={HEALTH[health].status}>{HEALTH[health].label}</StatusTag>;
}

export const configureHref = (app: FactoryAppId) => `#/factory/${app}/configure`;

export type Query<T> =
  | { state: 'loading' }
  | { state: 'error'; code: string; message: string }
  | { state: 'ready'; data: T };

/** Runs a factory op on mount and when its params change; `reload` runs it again. */
export function useQuery<T>(
  desktop: LoamsDesktopApi,
  app: FactoryAppId,
  op: string,
  params: Record<string, unknown> = {},
  enabled = true,
): [Query<T>, () => void] {
  const [value, setValue] = useState<Query<T>>({ state: 'loading' });
  const [tick, setTick] = useState(0);
  const key = JSON.stringify(params);
  const paramsRef = useRef(params);
  paramsRef.current = params;
  // biome-ignore lint/correctness/useExhaustiveDependencies: key and tick drive re-runs
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    setValue({ state: 'loading' });
    desktop.factory
      .query<T>({ app, op, params: paramsRef.current })
      .then((r) => {
        if (!live) return;
        setValue(
          r.ok
            ? { state: 'ready', data: r.value }
            : { state: 'error', code: r.code, message: r.message },
        );
      })
      .catch((e) => {
        if (live)
          setValue({
            state: 'error',
            code: 'ipc',
            message: e instanceof Error ? e.message : String(e),
          });
      });
    return () => {
      live = false;
    };
  }, [desktop, app, op, key, tick, enabled]);
  return [value, useCallback(() => setTick((t) => t + 1), [])];
}

export function QueryError({
  app,
  label,
  code,
  message,
}: {
  app: FactoryAppId;
  label?: string;
  code: string;
  message: string;
}) {
  return (
    <div className="p-4">
      <Notice tone="danger" title={message}>
        {code === 'auth_failed' ? (
          <a href={configureHref(app)}>Update credentials</a>
        ) : code === 'unconfigured' ? (
          <a href={configureHref(app)}>Configure {label ?? app}</a>
        ) : undefined}
      </Notice>
    </div>
  );
}

/** A panel card for one query: refresh button, loading, error and the data. */
export function QueryCard<T>({
  app,
  title,
  query,
  className,
  actions,
  children,
}: {
  app: FactoryAppId;
  title: string;
  query: [Query<T>, () => void];
  className?: string;
  actions?: ReactNode;
  children: (data: T) => ReactNode;
}) {
  const [q, reload] = query;
  return (
    <Card
      title={title}
      flush
      className={className}
      actions={
        <span className="flex items-center gap-2">
          {actions}
          <Button
            size="icon"
            variant="quiet"
            aria-label={`Refresh ${title}`}
            title={`Refresh ${title}`}
            onClick={reload}
          >
            <RefreshCw aria-hidden="true" size={14} />
          </Button>
        </span>
      }
    >
      {q.state === 'loading' ? (
        <p className="p-4 text-sm text-muted">Loading…</p>
      ) : q.state === 'error' ? (
        <QueryError app={app} label={APP_LABELS[app]} code={q.code} message={q.message} />
      ) : (
        children(q.data)
      )}
    </Card>
  );
}

export const infoOf = (apps: FactoryAppInfo[], app: FactoryAppId) => apps.find((a) => a.id === app);

export function PageHead({
  title,
  subtitle,
  actions,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="lc-page-head flex flex-wrap items-start justify-between gap-4">
      <div>
        <h1>{title}</h1>
        {subtitle && <p>{subtitle}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}

export const ago = (iso: string | undefined, fmt: (s: string) => string) => {
  if (!iso) return '';
  return Number.isNaN(Date.parse(iso)) ? iso : fmt(iso);
};
