// The two live charts: request/error rate, and resource usage per product
// (stacked area, one resource at a time). Both seed from a snapshot and then
// append from a stream; `LiveChart` keeps the time window and the table
// fallback screen readers get.

import { LiveChart } from '@loams/charts';
import { Card, Notice, Segmented } from '@loams/ui';
import { useMemo, useState } from 'react';
import { useLoad } from '../load.js';
import { trafficSpec, usageSpec } from './specs.js';
import type { OverviewMetricsClient, Resource } from './types.js';

export const CHART_WINDOW_MS = 10 * 60_000;

export function TrafficChart({ client }: { client: OverviewMetricsClient }) {
  const history = useLoad(() => client.trafficHistory(), client);
  const stream = useMemo(() => client.trafficStream(), [client]);
  return (
    <div data-panel="traffic">
      <Card title="Request and error rate">
        {history.state === 'error' ? (
          <Notice tone="danger" title="Not reachable">
            {history.message}
          </Notice>
        ) : (
          <LiveChart
            spec={trafficSpec}
            data={history.state === 'ok' ? history.data : []}
            stream={stream}
            windowMs={CHART_WINDOW_MS}
            height={240}
            ariaLabel="Requests and errors per second, last 10 minutes"
          />
        )}
      </Card>
    </div>
  );
}

const RESOURCES: { id: Resource; label: string }[] = [
  { id: 'cpu', label: 'CPU' },
  { id: 'memory', label: 'Memory' },
  { id: 'storage', label: 'Storage' },
];

export function UsageChart({ client }: { client: OverviewMetricsClient }) {
  const [resource, setResource] = useState<Resource>('cpu');
  // Key and scope on both: a new client or a new resource drops the old history
  // rather than pairing it with the new stream.
  const scope = useMemo(() => ({ client, resource }), [client, resource]);
  const history = useLoad(() => client.usageHistory(resource), scope, true, scope);
  const stream = useMemo(() => client.usageStream(resource), [client, resource]);
  const spec = useMemo(() => usageSpec(resource), [resource]);
  const label = RESOURCES.find((r) => r.id === resource)?.label ?? resource;
  return (
    <div data-panel="usage">
      <Card
        title="Resource usage"
        actions={
          // A segmented toggle, not tabs: `aria-pressed` says which resource is showing.
          <Segmented
            label="Resource"
            value={resource}
            onValueChange={setResource}
            options={RESOURCES.map((r) => ({ value: r.id, label: r.label }))}
          />
        }
      >
        {history.state === 'error' ? (
          <Notice tone="danger" title="Not reachable">
            {history.message}
          </Notice>
        ) : (
          <LiveChart
            // A new resource is a new series set: remount rather than mix them.
            key={resource}
            spec={spec}
            data={history.state === 'ok' ? history.data : []}
            stream={stream}
            windowMs={CHART_WINDOW_MS}
            height={240}
            ariaLabel={`${label} usage per product, stacked, last 10 minutes`}
          />
        )}
      </Card>
    </div>
  );
}
