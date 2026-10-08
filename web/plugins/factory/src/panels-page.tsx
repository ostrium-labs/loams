import type { FactoryAppId, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Empty } from '@loams/ui';
import { type ReactNode, useState } from 'react';
import { useApps } from './home.js';
import { configureHref, HealthPill, PageHead, ROLES } from './model.js';
import { ForgejoPanel } from './panels/forgejo.js';
import { GlitchtipPanel } from './panels/glitchtip.js';
import { LangfusePanel } from './panels/langfuse.js';
import { MatomoPanel } from './panels/matomo.js';
import { OpenPanelPanel } from './panels/openpanel.js';
import { PlanePanel } from './panels/plane.js';
import { ZulipPanel } from './panels/zulip.js';

const PANELS: Partial<Record<FactoryAppId, (p: { desktop: LoamsDesktopApi }) => ReactNode>> = {
  forgejo: ForgejoPanel,
  zulip: ZulipPanel,
  plane: PlanePanel,
  glitchtip: GlitchtipPanel,
  openpanel: OpenPanelPanel,
  matomo: MatomoPanel,
  langfuse: LangfusePanel,
};

/** `/factory/:app`: the app's native panels, or "Full UI only". */
export function PanelsPage({
  desktop,
  app,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  app: FactoryAppId;
  navigate: (to: string) => void;
}) {
  const { apps } = useApps(desktop);
  const [openError, setOpenError] = useState<string>();
  if (!apps) {
    return (
      <div className="lc-page">
        <p className="text-sm text-muted">Loading…</p>
      </div>
    );
  }
  const info = apps.find((a) => a.id === app);
  if (!info) {
    return (
      <div className="lc-page">
        <Empty
          title="Unknown app"
          actions={<Button onClick={() => navigate('/factory')}>Back</Button>}
        />
      </div>
    );
  }
  const open = async () => {
    setOpenError(undefined);
    try {
      const r = await desktop.factory.openApp(app);
      if (!r.ok) setOpenError(r.message);
    } catch (e) {
      setOpenError(e instanceof Error ? e.message : String(e));
    }
  };
  const Panel = PANELS[app];
  const configured = info.health !== 'unconfigured';
  return (
    <div className="lc-page">
      <PageHead
        title={
          <span className="flex items-center gap-3">
            {info.label} <HealthPill health={info.health} />
          </span>
        }
        subtitle={`${ROLES[app]}${info.url ? ` · ${info.url}` : ''}`}
        actions={
          <>
            <Button size="sm" variant="quiet" onClick={() => navigate('/factory')}>
              All apps
            </Button>
            <Button size="sm" onClick={() => navigate(`/factory/${app}/configure`)}>
              Configure
            </Button>
            <Button size="sm" variant="primary" disabled={!configured} onClick={() => void open()}>
              Open app
            </Button>
          </>
        }
      />
      {openError && <p className="mb-4 text-sm text-danger">{openError}</p>}
      {!configured ? (
        <Empty
          title={`${info.label} is not configured`}
          actions={
            <a className="loams-btn loams-btn-primary" href={configureHref(app)}>
              Configure {info.label}
            </a>
          }
        >
          Add its URL and credentials to see its data here.
        </Empty>
      ) : !Panel ? (
        <Empty title="Full UI only">
          {info.label} has no native panels. Use Open app to work in its own interface.
        </Empty>
      ) : (
        <Panel desktop={desktop} />
      )}
    </div>
  );
}
