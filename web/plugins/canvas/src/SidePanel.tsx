// The details panel a node opens: status, facts, a live chart of its key metric, its
// dependencies both ways, and a deep link to the service's product page.

import { LiveChart } from '@loams/charts';
import { Badge, Button, StatusTag } from '@loams/ui';
import { type ComponentProps, useEffect, useId, useMemo, useRef } from 'react';
import type { Service, ServiceEdge } from './client.js';
import { kindInfo, STATUS_LABEL, STATUS_TONE, serviceHref } from './kinds.js';
import { useServiceMetric } from './ServiceNode.js';

type ChartSpec = ComponentProps<typeof LiveChart>['spec'];

/** The panel chart's spec: the service's key metric over time, as a line. */
export function metricSpec(s: Service): ChartSpec {
  const m = kindInfo(s.kind).metric;
  return {
    chartType: 'Line Chart',
    title: `${m.name} (${m.unit})`,
    encodings: {
      x: { field: 'time', type: 'temporal' },
      y: { field: 'value', type: 'quantitative' },
    },
  } as ChartSpec;
}

/** How the other end of `e` relates to this service, read from this service's side. */
export function relation(e: ServiceEdge, dir: 'in' | 'out'): string {
  if (e.kind === 'depends-on') return dir === 'out' ? 'depends on' : 'needed by';
  if (e.label) return dir === 'out' ? e.label : `${e.label} this`;
  return dir === 'out' ? 'flows to' : 'flows from';
}

export function SidePanel({
  service,
  services,
  edges,
  onClose,
  onNavigate,
  onFocusService,
}: {
  service: Service;
  services: readonly Service[];
  edges: readonly ServiceEdge[];
  onClose: () => void;
  onNavigate: (href: string) => void;
  onFocusService: (id: string) => void;
}) {
  const info = kindInfo(service.kind);
  const headingId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const points = useServiceMetric(service.id);
  const rows = useMemo(() => points.map((p) => ({ time: p.t, value: p.v })), [points]);
  const spec = useMemo(() => metricSpec(service), [service]);
  const byId = useMemo(() => new Map(services.map((s) => [s.id, s])), [services]);
  const incoming = edges.filter((e) => e.target === service.id);
  const outgoing = edges.filter((e) => e.source === service.id);

  // Focus follows the panel open, so keyboard and screen reader users land in it.
  // oxlint-disable-next-line react/exhaustive-deps -- refocus whenever the service changes
  useEffect(() => {
    heading.current?.focus();
  }, [service.id]);

  const link = (e: ServiceEdge, dir: 'in' | 'out') => {
    const s = byId.get(dir === 'in' ? e.source : e.target);
    if (!s) return null;
    return (
      <li key={e.id} className="flex items-center gap-2">
        <Button
          variant="quiet"
          size="sm"
          onClick={() => onFocusService(s.id)}
          aria-label={`Go to ${kindInfo(s.kind).label} ${s.name}`}
        >
          <span className={/* mono-ok: id */ 'font-mono'}>{s.name}</span>
        </Button>
        <span className="text-xs text-faint">
          {kindInfo(s.kind).label} · {relation(e, dir)}
        </span>
      </li>
    );
  };

  return (
    <aside
      aria-labelledby={headingId}
      className="lc-canvas-panel flex w-full flex-col gap-4 border border-rule bg-surface p-4 xl:w-[340px]"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="text-xs uppercase tracking-wide text-faint">{info.label}</div>
          <h2
            id={headingId}
            ref={heading}
            tabIndex={-1}
            className={/* mono-ok: id */ 'm-0 truncate font-mono text-base'}
          >
            {service.name}
          </h2>
        </div>
        <Button variant="quiet" size="icon" aria-label="Close details" onClick={onClose}>
          ×
        </Button>
      </div>
      <div className="flex items-center gap-2">
        <StatusTag status={STATUS_TONE[service.status]}>{STATUS_LABEL[service.status]}</StatusTag>
        <Badge title="Service id">{service.id}</Badge>
      </div>

      {service.details.length > 0 && (
        <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
          {service.details.map((d) => (
            <div key={d.label} className="contents">
              <dt className="text-muted">{d.label}</dt>
              <dd className={/* mono-ok: id */ 'm-0 font-mono'}>{d.value}</dd>
            </div>
          ))}
        </dl>
      )}

      <section aria-label={`${info.metric.name} chart`}>
        <LiveChart
          spec={spec}
          data={rows}
          windowMs={2 * 60 * 1000}
          height={160}
          ariaLabel={`${info.metric.name} (${info.metric.unit}) for ${service.name}, last two minutes`}
        />
      </section>

      <section>
        <h3 className="m-0 mb-1 text-sm">Outgoing</h3>
        {outgoing.length ? (
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {outgoing.map((e) => link(e, 'out'))}
          </ul>
        ) : (
          <p className="m-0 text-sm text-muted">No outgoing connections.</p>
        )}
      </section>
      <section>
        <h3 className="m-0 mb-1 text-sm">Incoming</h3>
        {incoming.length ? (
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {incoming.map((e) => link(e, 'in'))}
          </ul>
        ) : (
          <p className="m-0 text-sm text-muted">No incoming connections.</p>
        )}
      </section>

      <Button variant="primary" onClick={() => onNavigate(serviceHref(service))}>
        Open in {info.product}
      </Button>
    </aside>
  );
}
