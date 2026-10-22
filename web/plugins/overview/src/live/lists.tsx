// Recent deployments and open alerts: plain tables, states in words as well
// as colour, times as relative text with the exact time in a <time> element.

import { Card, Empty, formatRelative, Notice, type Status, StatusTag, Table } from '@loams/ui';
import type { ReactNode } from 'react';
import { type Load, useLoad } from '../load.js';
import type {
  AlertItem,
  Deployment,
  DeploymentState,
  OverviewMetricsClient,
  Severity,
} from './types.js';

const DEPLOY_TONE: Record<DeploymentState, { status: Status; label: string }> = {
  live: { status: 'done', label: 'Live' },
  building: { status: 'progress', label: 'Building' },
  failed: { status: 'failed', label: 'Failed' },
  'rolled-back': { status: 'planned', label: 'Rolled back' },
};

const SEVERITY_TONE: Record<Severity, { status: Status; label: string }> = {
  critical: { status: 'failed', label: 'Critical' },
  warning: { status: 'progress', label: 'Warning' },
  info: { status: 'neutral', label: 'Info' },
};

const ALERT_STATE: Record<AlertItem['state'], string> = {
  firing: 'Firing',
  acknowledged: 'Acknowledged',
  resolved: 'Resolved',
};

function When({ at, now }: { at: number; now: number }) {
  const iso = new Date(at).toISOString();
  return (
    <time dateTime={iso} title={iso}>
      {formatRelative(iso, now)}
    </time>
  );
}

function Loaded<T>({ load, children }: { load: Load<T>; children: (data: T) => ReactNode }) {
  if (load.state === 'loading') return <p className="text-body text-muted m-0">Loading</p>;
  if (load.state === 'error')
    return (
      <Notice tone="danger" title="Not reachable">
        {load.message}
      </Notice>
    );
  return <>{children(load.data)}</>;
}

export function DeploymentsPanel({
  client,
  now = Date.now,
}: {
  client: OverviewMetricsClient;
  now?: () => number;
}) {
  const load = useLoad(() => client.deployments(6), client);
  return (
    <div data-panel="deployments">
      <Card title="Recent deployments" flush>
        <Loaded load={load}>
          {(rows: Deployment[]) => (
            <Table
              caption="Recent deployments"
              rows={rows}
              rowKey={(d) => d.id}
              empty={
                <Empty title="No deployments yet">Deployments show here as they roll out.</Empty>
              }
              columns={[
                { key: 'service', header: 'Service', cell: (d) => d.service },
                { key: 'version', header: 'Version', cell: (d) => <code>{d.version}</code> },
                {
                  key: 'state',
                  header: 'State',
                  cell: (d) => (
                    <StatusTag status={DEPLOY_TONE[d.state].status}>
                      {DEPLOY_TONE[d.state].label}
                    </StatusTag>
                  ),
                },
                { key: 'by', header: 'By', cell: (d) => d.by },
                { key: 'at', header: 'When', cell: (d) => <When at={d.at} now={now()} /> },
              ]}
            />
          )}
        </Loaded>
      </Card>
    </div>
  );
}

export function AlertsPanel({
  client,
  now = Date.now,
}: {
  client: OverviewMetricsClient;
  now?: () => number;
}) {
  const load = useLoad(() => client.alerts(), client);
  const firing = load.state === 'ok' ? load.data.filter((a) => a.state === 'firing').length : 0;
  return (
    <div data-panel="alerts">
      <Card
        title="Alerts and incidents"
        flush
        actions={
          load.state === 'ok' && (
            <StatusTag status={firing > 0 ? 'failed' : 'done'}>
              {firing > 0 ? `${firing} firing` : 'All clear'}
            </StatusTag>
          )
        }
      >
        <Loaded load={load}>
          {(rows: AlertItem[]) => (
            <Table
              caption="Alerts and incidents"
              rows={rows}
              rowKey={(a) => a.id}
              empty={<Empty title="No alerts">Nothing is firing.</Empty>}
              columns={[
                {
                  key: 'severity',
                  header: 'Severity',
                  cell: (a) => (
                    <StatusTag status={SEVERITY_TONE[a.severity].status}>
                      {SEVERITY_TONE[a.severity].label}
                    </StatusTag>
                  ),
                },
                { key: 'title', header: 'Alert', cell: (a) => a.title },
                { key: 'service', header: 'Service', cell: (a) => a.service },
                { key: 'state', header: 'State', cell: (a) => ALERT_STATE[a.state] },
                { key: 'since', header: 'Since', cell: (a) => <When at={a.since} now={now()} /> },
              ]}
            />
          )}
        </Loaded>
      </Card>
    </div>
  );
}
