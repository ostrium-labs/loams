import type { FactoryAppInfo, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Card, Empty, Notice } from '@loams/ui';
import { RefreshCw } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { HealthPill, isConfigured, PageHead, ROLES } from './model.js';

function Tile({
  desktop,
  app,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  app: FactoryAppInfo;
  navigate: (to: string) => void;
}) {
  const [error, setError] = useState<string>();
  const configured = isConfigured(app.health);
  const open = async () => {
    setError(undefined);
    try {
      const r = await desktop.factory.openApp(app.id);
      if (!r.ok) setError(r.message);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <article aria-label={app.label} className="min-w-0">
      <Card title={app.label} actions={<HealthPill health={app.health} />} className="h-full">
        <div className="flex flex-col gap-3">
          <p className="text-sm text-muted">{ROLES[app.id]}</p>
          <p className="truncate font-mono text-xs text-faint" title={app.url}>
            {app.url ?? 'No URL yet'}
          </p>
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant={configured ? 'secondary' : 'primary'}
              onClick={() => navigate(`/factory/${app.id}/configure`)}
            >
              Configure
            </Button>
            <Button
              size="sm"
              disabled={!configured}
              onClick={() => navigate(`/factory/${app.id}/app`)}
            >
              Open app
            </Button>
            <Button size="sm" variant="quiet" disabled={!configured} onClick={() => void open()}>
              Open in new window
            </Button>
            {app.hasPanels && (
              <Button
                size="sm"
                variant="quiet"
                disabled={!configured}
                onClick={() => navigate(`/factory/${app.id}`)}
              >
                Panels
              </Button>
            )}
          </div>
          {error && <p className="text-sm text-danger">{error}</p>}
        </div>
      </Card>
    </article>
  );
}

export function useApps(desktop: LoamsDesktopApi) {
  const [apps, setApps] = useState<FactoryAppInfo[]>();
  const [error, setError] = useState<string>();
  const reload = useCallback(() => {
    desktop.factory
      .list()
      .then((a) => {
        setApps(a);
        setError(undefined);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, [desktop]);
  useEffect(reload, [reload]);
  return { apps, error, reload };
}

/** `/factory`: the Software Factory hub, one tile per app. */
export function FactoryHome({
  desktop,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  navigate: (to: string) => void;
}) {
  const { apps, error, reload } = useApps(desktop);
  return (
    <div className="lc-page">
      <PageHead
        title="Software Factory"
        subtitle="The apps your team builds with: connect them once, then read their data here."
        actions={
          <Button variant="quiet" size="sm" onClick={reload}>
            <RefreshCw aria-hidden="true" size={14} /> Refresh
          </Button>
        }
      />
      <div className="flex flex-col gap-4">
        {apps?.some((a) => !a.persistent) && (
          <Notice
            tone="warn"
            title="Credentials are kept for this session only: no system keychain found."
          />
        )}
        {error && <Notice tone="danger" title={error} />}
        {!apps && !error && <p className="text-sm text-muted">Loading…</p>}
        {apps && apps.length === 0 && (
          <Empty title="No factory apps">This build has no factory apps.</Empty>
        )}
        {apps && (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(18rem,1fr))] gap-4">
            {apps.map((a) => (
              <Tile key={a.id} desktop={desktop} app={a} navigate={navigate} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
