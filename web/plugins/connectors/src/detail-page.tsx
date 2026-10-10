import type { ConnectorDetail, ConnectorSummary, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Badge, Button, Card, Notice, StatusTag } from '@loams/ui';
import { useEffect, useState } from 'react';
import { statusTone, useCatalog } from './catalog-page.js';
import { ConfigureForm } from './configure-form.js';
import { PageHead } from './page-head.js';

type Rec = Record<string, unknown>;
const rec = (v: unknown): Rec => (typeof v === 'object' && v !== null ? (v as Rec) : {});

function Row({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[10rem_1fr] gap-3 border-b border-rule-soft py-2 text-sm last:border-b-0">
      <dt className="text-muted">{k}</dt>
      <dd className="m-0 min-w-0">{children}</dd>
    </div>
  );
}

const badges = (xs: unknown) =>
  Array.isArray(xs) && xs.length > 0 ? (
    <span className="flex flex-wrap gap-1">
      {xs.map((x) => (
        <Badge key={String(x)}>{String(x)}</Badge>
      ))}
    </span>
  ) : (
    <span className="text-muted">none</span>
  );

function Capabilities({ summary, manifest }: { summary: ConnectorSummary; manifest: Rec }) {
  const caps = rec(manifest.capabilities);
  const delivery = rec(caps.delivery);
  return (
    <Card title="Capabilities">
      <dl className="m-0">
        <Row k="Source">
          {summary.source ? (
            badges(summary.source)
          ) : (
            <span className="text-muted">no source side</span>
          )}
        </Row>
        <Row k="Sink">
          {summary.sink ? badges(summary.sink) : <span className="text-muted">no sink side</span>}
        </Row>
        <Row k="Delivery">
          {delivery.source || delivery.sink ? (
            <span className="font-mono text-xs">
              source {String(delivery.source ?? 'n/a')}, sink {String(delivery.sink ?? 'n/a')}
            </span>
          ) : (
            <span className="text-muted">n/a</span>
          )}
        </Row>
        <Row k="Ordering">{String(caps.ordering ?? 'n/a')}</Row>
        <Row k="Formats">{badges(caps.formats)}</Row>
        <Row k="Backpressure">{String(caps.backpressure ?? 'n/a')}</Row>
      </dl>
    </Card>
  );
}

function Facts({ summary, manifest }: { summary: ConnectorSummary; manifest: Rec }) {
  const lic = rec(manifest.licence);
  const deps = rec(lic.dependencies);
  const runtime = rec(manifest.runtime);
  return (
    <Card title="Auth, licence and runtime">
      <dl className="m-0">
        <Row k="Auth">{badges(summary.auth)}</Row>
        <Row k="Secrets">{badges(manifest.secrets)}</Row>
        <Row k="Licence">
          <span className="font-mono text-xs">{summary.licence || 'n/a'}</span>
          {Object.keys(deps).length > 0 && (
            <span className="ml-2 text-xs text-muted">
              {Object.entries(deps)
                .map(([k, v]) => `${k}: ${String(v)}`)
                .join(', ')}
            </span>
          )}
        </Row>
        <Row k="Runtime">
          <span className="font-mono text-xs">
            {summary.runtime.kind} {summary.runtime.ref}
          </span>
          {typeof runtime.version === 'string' && (
            <span className="ml-2 text-xs text-muted">{runtime.version}</span>
          )}
        </Row>
        <Row k="Spec version">{String(manifest.specVersion ?? 'n/a')}</Row>
      </dl>
    </Card>
  );
}

/** `/connectors/:id`: capabilities, auth, licence, runtime and the Configure form. */
export function DetailPage({
  desktop,
  id,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  id: string;
  navigate: (to: string) => void;
}) {
  const catalog = useCatalog(desktop);
  const [detail, setDetail] = useState<
    | { state: 'loading' }
    | { state: 'error'; message: string }
    | { state: 'ready'; data: ConnectorDetail }
  >({ state: 'loading' });
  useEffect(() => {
    let live = true;
    setDetail({ state: 'loading' });
    desktop.connectors
      .get(id)
      .then(
        (r) =>
          live &&
          setDetail(
            r.ok ? { state: 'ready', data: r.value } : { state: 'error', message: r.message },
          ),
      )
      .catch(
        (e) =>
          live &&
          setDetail({ state: 'error', message: e instanceof Error ? e.message : String(e) }),
      );
    return () => {
      live = false;
    };
  }, [desktop, id]);

  const summary = catalog.state === 'ready' ? catalog.data.find((c) => c.id === id) : undefined;
  const back = (
    <Button size="sm" variant="quiet" onClick={() => navigate('/connectors')}>
      All connectors
    </Button>
  );
  if (detail.state === 'error' || (catalog.state === 'ready' && !summary)) {
    return (
      <div className="lc-page">
        <PageHead title="Connector" actions={back} />
        <Notice
          tone="danger"
          title={detail.state === 'error' ? detail.message : `No connector "${id}".`}
        />
      </div>
    );
  }
  if (detail.state === 'loading' || !summary) {
    return (
      <div className="lc-page">
        <PageHead title="Connector" actions={back} />
        <p className="text-sm text-muted">
          {catalog.state === 'error' ? catalog.message : 'Loading…'}
        </p>
      </div>
    );
  }
  const manifest = detail.data.manifest;
  return (
    <div className="lc-page">
      <PageHead
        title={summary.name}
        subtitle={
          <>
            <span className="font-mono">{summary.id}</span> · {summary.category}{' '}
            <StatusTag status={statusTone(summary.status)}>{summary.status}</StatusTag>
          </>
        }
        actions={
          <>
            {back}
            <Button variant="primary" disabled title="Connector runtime not yet available (CN1)">
              Run
            </Button>
          </>
        }
      />
      <div className="flex flex-col gap-4">
        <p className="m-0 text-sm text-muted">Connector runtime not yet available (CN1).</p>
        {summary.stub && (
          <Notice title="Planned connector">
            This connector is a generated placeholder: no capability detail or config fields yet.
          </Notice>
        )}
        <div className="grid gap-4 lg:grid-cols-2">
          <Capabilities summary={summary} manifest={manifest} />
          <Facts summary={summary} manifest={manifest} />
        </div>
        <ConfigureForm desktop={desktop} summary={summary} detail={detail.data} />
      </div>
    </div>
  );
}
