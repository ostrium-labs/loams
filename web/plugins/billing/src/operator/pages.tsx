// The operator view of Loams Meter (design 63 §18, D555; ME1 Task 28): Meters, Plans,
// Customers (search, a customer's balances and entitlements, manual grants with a reason) and
// Meter health (owners and epochs, leases, reconcile drift, pipe lag, close-job runs, exporter
// cursors, dropped events). Each section is its own route under /operator.

import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  Field,
  Input,
  Notice,
  Primary,
  SampleDataNotice,
  SearchInput,
  Select,
  Stat,
  Stats,
  StatusTag,
  Table,
  Tabs,
  Textarea,
} from '@loams/ui';
import { type ReactNode, useState } from 'react';
import { EntitlementCard } from '../entitlements.js';
import { formatAmount, formatMoney, formatTime, toNanos } from '../format.js';
import { errorText, idempotencyKey, Loaded, useLoad } from '../load.js';
import { PAYMENT_LABEL } from '../usage.js';
import type {
  CloseRun,
  CustomerDetail,
  ExporterStatus,
  ManualGrant,
  MeterDef,
  OperatorClient,
  PartitionOwner,
  PlanTemplate,
} from './types.js';

export const OPERATOR_ROUTES = {
  meters: '/operator/meters',
  plans: '/operator/plans',
  customers: '/operator/customers',
  customer: (id: string) => `/operator/customers/${encodeURIComponent(id)}`,
  health: '/operator/meter-health',
} as const;

export type OperatorSection = 'meters' | 'plans' | 'customers' | 'health';

const SECTIONS: { value: OperatorSection; label: string; href: string }[] = [
  { value: 'meters', label: 'Meters', href: OPERATOR_ROUTES.meters },
  { value: 'plans', label: 'Plans', href: OPERATOR_ROUTES.plans },
  { value: 'customers', label: 'Customers', href: OPERATOR_ROUTES.customers },
  { value: 'health', label: 'Meter health', href: OPERATOR_ROUTES.health },
];

/** Drift above 1% alerts (§12, `drift_alert_above_one_percent`). */
export const DRIFT_ALERT = 0.01;
/** Pipe lag alerts at 15 minutes (§14). */
export const PIPE_LAG_ALERT_SECONDS = 15 * 60;
/** `loams_meter_hard_limit_overrun_ratio` alerts at 5% (§14). */
export const OVERRUN_ALERT = 0.05;

export function OperatorLayout({
  section,
  navigate,
  sample,
  children,
}: {
  section: OperatorSection;
  navigate: (to: string) => void;
  sample?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="lc-page">
      <header className="lc-page-head">
        <div>
          <h1>Loams Meter</h1>
          <p>
            Meters, plans and customers, and the health of counting and export. Needs meter_admin.
          </p>
        </div>
      </header>
      {sample && (
        <div className="mb-3">
          <SampleDataNotice>Meters, plans, customers and health are fixtures.</SampleDataNotice>
        </div>
      )}
      <Tabs
        label="Meter sections"
        value={section}
        onValueChange={(v) => {
          const to = SECTIONS.find((s) => s.value === v)?.href;
          if (to) navigate(to);
        }}
        tabs={SECTIONS.map((s) => ({ value: s.value, label: s.label }))}
      />
      <div className="pt-4">{children}</div>
    </div>
  );
}

/* Meters ------------------------------------------------------------------------------- */

export function MetersPage({ client }: { client: OperatorClient }) {
  const [load] = useLoad(() => client.meters(), [client]);
  return (
    <Loaded load={load} what="meters">
      {(meters) => (
        <Card title="Meters" flush>
          <Table
            caption="Meters"
            columns={[
              {
                key: 'slug',
                header: 'Meter',
                cell: (m: MeterDef) => <Primary title={m.slug} detail={m.description} />,
              },
              {
                key: 'type',
                header: 'Event type',
                cell: (m) => <code>{m.eventType}</code>,
              },
              { key: 'agg', header: 'Aggregation', cell: (m) => m.aggregation },
              {
                key: 'value',
                header: 'Value',
                cell: (m) => (m.valueProperty ? <code>{m.valueProperty}</code> : '—'),
              },
              {
                key: 'group',
                header: 'Group by',
                cell: (m) => Object.keys(m.groupBy).join(', ') || '—',
              },
              {
                key: 'events',
                header: 'Events, 24 h',
                numeric: true,
                cell: (m) => (m.events24h === undefined ? '—' : m.events24h.toLocaleString()),
              },
              {
                key: 'state',
                header: 'State',
                cell: (m) =>
                  m.archived ? (
                    <Badge variant="neutral">Archived</Badge>
                  ) : m.inUse ? (
                    <Badge variant="success" title="Semantics are fixed once events were counted">
                      In use
                    </Badge>
                  ) : (
                    <Badge variant="outline">Unused</Badge>
                  ),
              },
            ]}
            rows={meters}
            rowKey={(m) => m.slug}
            empty={<p className="p-4 text-muted">No meters defined.</p>}
          />
        </Card>
      )}
    </Loaded>
  );
}

/* Plans -------------------------------------------------------------------------------- */

function templateText(t: PlanTemplate): string {
  switch (t.kind.type) {
    case 'metered':
      return `${t.kind.mode.replace(/_/g, ' ')}, per ${t.kind.interval}${
        t.kind.issueAfterReset ? `, ${formatAmount(t.kind.issueAfterReset, t.unit)}` : ''
      }${t.kind.overagePrice ? `, overage ${formatMoney(t.kind.overagePrice)} per unit` : ''}`;
    case 'boolean':
      return t.kind.enabled ? 'on' : 'off';
    case 'static':
      return Object.entries(t.kind.config)
        .map(([k, v]) => `${k}: ${String(v)}`)
        .join(', ');
  }
}

export function PlansPage({ client }: { client: OperatorClient }) {
  const [load] = useLoad(() => client.plans(), [client]);
  return (
    <Loaded load={load} what="plans">
      {(plans) => (
        <div className="grid gap-4">
          <p className="text-muted">
            Plans are applied from YAML through the GitOps reconciler; each change is a new version,
            and customers move at their next period.
          </p>
          {plans.map((p) => (
            <Card key={p.id} title={`${p.name} (${p.id})`}>
              {p.versions.map((v, i) => (
                <div key={v.version} className="mb-4">
                  <div className="mb-2 flex flex-wrap items-center gap-2">
                    <strong>Version {v.version}</strong>
                    {i === 0 && <Badge variant="success">Current</Badge>}
                    <span className="text-small text-muted">
                      Applied {formatTime(v.appliedAt)} by {v.appliedBy}; {v.customers} customers
                    </span>
                  </div>
                  <Table
                    caption={`${p.name} version ${v.version}`}
                    columns={[
                      {
                        key: 'feature',
                        header: 'Feature',
                        cell: (t: PlanTemplate) => t.feature,
                      },
                      { key: 'kind', header: 'Kind', cell: (t) => t.kind.type },
                      { key: 'terms', header: 'Terms', cell: templateText },
                    ]}
                    rows={v.entitlements}
                    rowKey={(t) => t.feature}
                  />
                </div>
              ))}
            </Card>
          ))}
        </div>
      )}
    </Loaded>
  );
}

/* Customers ---------------------------------------------------------------------------- */

export function CustomersPage({
  client,
  navigate,
}: {
  client: OperatorClient;
  navigate: (to: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [load] = useLoad(() => client.customers(query), [client, query]);
  return (
    <div className="grid gap-4">
      <SearchInput
        value={query}
        onValueChange={setQuery}
        placeholder="Name, subject, id or provider id"
        label="Search customers"
      />
      <Loaded load={load} what="customers">
        {(rows) => (
          <Card title="Customers" flush>
            <Table
              caption="Customers"
              columns={[
                {
                  key: 'name',
                  header: 'Customer',
                  cell: (c) => (
                    <Button variant="link" onClick={() => navigate(OPERATOR_ROUTES.customer(c.id))}>
                      {c.name}
                    </Button>
                  ),
                },
                {
                  key: 'subjects',
                  header: 'Subjects',
                  cell: (c) => c.subjects.join(', '),
                },
                {
                  key: 'plan',
                  header: 'Plan',
                  cell: (c) => (c.plan ? `${c.plan.id} v${c.plan.version}` : '—'),
                },
                {
                  key: 'payment',
                  header: 'Payment',
                  cell: (c) => PAYMENT_LABEL[c.paymentState],
                },
                {
                  key: 'stopped',
                  header: 'Stopped',
                  cell: (c) =>
                    c.stopped.length ? <Badge variant="danger">{c.stopped.join(', ')}</Badge> : '—',
                },
              ]}
              rows={rows}
              rowKey={(c) => c.id}
              onRowClick={(c) => navigate(OPERATOR_ROUTES.customer(c.id))}
              empty={<p className="p-4 text-muted">No customer matches.</p>}
            />
          </Card>
        )}
      </Loaded>
    </div>
  );
}

function GrantForm({
  customer,
  client,
  onDone,
}: {
  customer: CustomerDetail;
  client: OperatorClient;
  onDone: () => void;
}) {
  const metered = customer.entitlements.filter((e) => e.kind.type === 'metered');
  const [entitlement, setEntitlement] = useState(metered[0]?.id ?? '');
  const [amount, setAmount] = useState('');
  const [kind, setKind] = useState<ManualGrant['kind']>('promotional');
  const [days, setDays] = useState('30');
  const [reason, setReason] = useState('');
  const [confirming, setConfirming] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const [key, setKey] = useState(idempotencyKey);
  const target = metered.find((e) => e.id === entitlement);
  const amountOk = /^\d+(?:\.\d{1,9})?$/.test(amount.trim()) && toNanos(amount.trim() || '0') > 0n;
  const daysOk = days.trim() === '' || /^\d+$/.test(days.trim());
  const ready = !!target && amountOk && daysOk && reason.trim().length > 0;
  if (metered.length === 0) {
    return <p className="text-muted">This customer has no metered entitlement to grant on.</p>;
  }
  const grant: ManualGrant = {
    entitlement,
    amount: amount.trim(),
    kind,
    expiresInDays: days.trim() ? Number(days) : undefined,
    reason: reason.trim(),
  };
  return (
    <div className="grid gap-3">
      {error && (
        <Notice tone="danger" title="Grant not added">
          {error}
        </Notice>
      )}
      <Field label="Entitlement">
        {(p) => (
          <Select {...p} value={entitlement} onChange={(e) => setEntitlement(e.target.value)}>
            {metered.map((e) => (
              <option key={e.id} value={e.id}>
                {e.label}
              </option>
            ))}
          </Select>
        )}
      </Field>
      <Field
        label={`Amount (raw ${target?.unit ?? ''})`}
        hint={amountOk && target ? formatAmount(amount.trim(), target.unit) : undefined}
        error={amount && !amountOk ? 'A positive decimal, up to 9 decimals.' : undefined}
      >
        {(p) => (
          <Input
            {...p}
            inputMode="decimal"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
          />
        )}
      </Field>
      <Field label="Kind">
        {(p) => (
          <Select
            {...p}
            value={kind}
            onChange={(e) => setKind(e.target.value as ManualGrant['kind'])}
          >
            <option value="promotional">Promotional</option>
            <option value="prepaid">Prepaid</option>
          </Select>
        )}
      </Field>
      <Field
        label="Expires after (days)"
        hint="Empty: never."
        error={daysOk ? undefined : 'Whole days.'}
      >
        {(p) => (
          <Input
            {...p}
            inputMode="numeric"
            value={days}
            onChange={(e) => setDays(e.target.value)}
          />
        )}
      </Field>
      <Field label="Reason" hint="Required; recorded in the audit log.">
        {(p) => <Textarea {...p} value={reason} onChange={(e) => setReason(e.target.value)} />}
      </Field>
      <div>
        <Button variant="primary" disabled={!ready} onClick={() => setConfirming(true)}>
          Add grant
        </Button>
      </div>
      <ConfirmDialog
        open={confirming}
        pending={pending}
        onClose={() => setConfirming(false)}
        object={`a grant for ${customer.name}`}
        confirm="Add grant"
        impact={
          target && amountOk
            ? `${formatAmount(grant.amount, target.unit)} of ${target.label}, ${kind}. Reason: “${grant.reason}”. The grant is audited.`
            : ''
        }
        onConfirm={async () => {
          setPending(true);
          try {
            await client.createGrant(customer.id, grant, key);
            setKey(idempotencyKey());
            setAmount('');
            setReason('');
            setError(undefined);
            onDone();
          } catch (e) {
            setError(errorText(e));
          } finally {
            setPending(false);
            setConfirming(false);
          }
        }}
      />
    </div>
  );
}

export function CustomerPage({
  id,
  client,
  navigate,
}: {
  id: string;
  client: OperatorClient;
  navigate: (to: string) => void;
}) {
  const [load, reload] = useLoad(() => client.customer(id), [client, id]);
  return (
    <Loaded load={load} what="the customer">
      {(c) => (
        <div className="grid gap-4">
          <div>
            <Button variant="link" onClick={() => navigate(OPERATOR_ROUTES.customers)}>
              All customers
            </Button>
          </div>
          <Stats>
            <Stat label="Customer" value={c.name} detail={c.id} />
            <Stat label="Subjects" value={c.subjects.join(', ')} />
            <Stat label="Plan" value={c.plan ? `${c.plan.id} v${c.plan.version}` : 'None'} />
            <Stat
              label="Payment"
              value={PAYMENT_LABEL[c.paymentState]}
              detail={Object.entries(c.providerIds)
                .map(([p, v]) => `${p}: ${v}`)
                .join(', ')}
            />
            <Stat
              label="Overage"
              value={c.overageEnabled ? 'On' : 'Off'}
              detail={c.spendCap ? `Cap ${formatMoney(c.spendCap)}` : undefined}
            />
          </Stats>
          <Card title="Balances" flush>
            <Table
              caption="Balances this period"
              columns={[
                { key: 'feature', header: 'Feature', cell: (b) => b.label },
                {
                  key: 'used',
                  header: 'Used',
                  numeric: true,
                  cell: (b) => formatAmount(b.usage, b.unit),
                },
                {
                  key: 'balance',
                  header: 'Balance',
                  numeric: true,
                  cell: (b) => formatAmount(b.balance, b.unit),
                },
                {
                  key: 'overage',
                  header: 'Overage',
                  numeric: true,
                  cell: (b) => formatAmount(b.overage, b.unit),
                },
                {
                  key: 'access',
                  header: 'Access',
                  cell: (b) =>
                    b.hasAccess ? (
                      <Badge variant="success">Allowed</Badge>
                    ) : (
                      <Badge variant="danger">Stopped</Badge>
                    ),
                },
              ]}
              rows={c.balances}
              rowKey={(b) => b.feature}
              empty={<p className="p-4 text-muted">No metered usage this period.</p>}
            />
          </Card>
          <Card title="Manual grant">
            <GrantForm customer={c} client={client} onDone={reload} />
          </Card>
          {c.entitlements.map((e) => (
            <EntitlementCard key={e.id} e={e} />
          ))}
        </div>
      )}
    </Loaded>
  );
}

/* Meter health ------------------------------------------------------------------------- */

const RUN_STATUS: Record<CloseRun['status'], 'done' | 'progress' | 'failed'> = {
  ok: 'done',
  running: 'progress',
  failed: 'failed',
};

const EXPORTER_BADGE: Record<
  ExporterStatus['status'],
  'success' | 'warning' | 'danger' | 'neutral'
> = {
  ok: 'success',
  retrying: 'warning',
  failed: 'danger',
  off: 'neutral',
};

const pct = (r: number) => `${(r * 100).toFixed(2)}%`;

export function HealthPage({ client }: { client: OperatorClient }) {
  const [load, reload] = useLoad(() => client.health(), [client]);
  return (
    <Loaded load={load} what="meter health">
      {(h) => {
        const leaderless = h.partitions.filter((p) => !p.owner);
        const drifting = h.drift.filter((d) => Math.abs(d.drift) > DRIFT_ALERT);
        const failedRuns = h.closeRuns.filter((r) => r.status === 'failed');
        const lagging = h.pipeLagSeconds > PIPE_LAG_ALERT_SECONDS;
        return (
          <div className="grid gap-4">
            <div className="flex items-center gap-2">
              <span className="text-small text-muted">As of {formatTime(h.asOf)}</span>
              <Button size="sm" onClick={reload}>
                Refresh
              </Button>
            </div>
            {leaderless.length > 0 && (
              <Notice tone="warn" title="Partitions without an owner">
                {leaderless.map((p) => p.partition).join(', ')}: gates spend their leases, then
                follow policy until a new owner rebuilds.
              </Notice>
            )}
            {drifting.length > 0 && (
              <Notice tone="warn" title="Reconcile drift above 1%">
                {drifting.map((d) => `${d.subject}/${d.feature} ${pct(d.drift)}`).join('; ')}
              </Notice>
            )}
            {h.droppedEvents24h > 0 && (
              <Notice tone="danger" title="Usage events dropped">
                {h.droppedEvents24h.toLocaleString()} in the last 24 hours: under-billing.
              </Notice>
            )}
            <Stats>
              <Stat
                label="Pipe lag"
                value={`${h.pipeLagRecords.toLocaleString()} records`}
                detail={`${h.pipeLagSeconds} s${lagging ? ' — above 15 min' : ''}`}
              />
              <Stat label="Dropped events, 24 h" value={h.droppedEvents24h.toLocaleString()} />
              <Stat
                label="Hard-limit overrun"
                value={pct(h.hardLimitOverrun)}
                detail={h.hardLimitOverrun >= OVERRUN_ALERT ? 'Above the 5% alert' : 'Alert at 5%'}
              />
              <Stat label="Failed close runs" value={String(failedRuns.length)} />
            </Stats>
            <Card title="Counter owners" flush>
              <Table
                caption="Counter owners by partition"
                columns={[
                  {
                    key: 'p',
                    header: 'Partition',
                    numeric: true,
                    cell: (p: PartitionOwner) => p.partition,
                  },
                  {
                    key: 'owner',
                    header: 'Owner',
                    cell: (p) => p.owner ?? <Badge variant="warning">none</Badge>,
                  },
                  {
                    key: 'epoch',
                    header: 'Epoch',
                    numeric: true,
                    cell: (p) => p.epoch,
                  },
                  {
                    key: 'leases',
                    header: 'Leases',
                    numeric: true,
                    cell: (p) => p.leases,
                  },
                  {
                    key: 'offset',
                    header: 'Committed offset',
                    numeric: true,
                    cell: (p) => p.committedOffset.toLocaleString(),
                  },
                  {
                    key: 'rebuild',
                    header: 'Last rebuild',
                    cell: (p) => (p.rebuildSeconds === undefined ? '—' : `${p.rebuildSeconds} s`),
                  },
                ]}
                rows={h.partitions}
                rowKey={(p) => String(p.partition)}
              />
            </Card>
            <Card title="Reconcile drift" flush>
              <Table
                caption="Reconcile drift"
                columns={[
                  { key: 'subject', header: 'Subject', cell: (d) => d.subject },
                  { key: 'feature', header: 'Feature', cell: (d) => d.feature },
                  {
                    key: 'drift',
                    header: 'Drift',
                    numeric: true,
                    cell: (d) =>
                      Math.abs(d.drift) > DRIFT_ALERT ? (
                        <Badge variant="warning">{pct(d.drift)}</Badge>
                      ) : (
                        pct(d.drift)
                      ),
                  },
                  {
                    key: 'at',
                    header: 'Checked',
                    cell: (d) => formatTime(d.checkedAt),
                  },
                ]}
                rows={h.drift}
                rowKey={(d) => `${d.subject}/${d.feature}`}
              />
            </Card>
            <Card title="Close-job runs" flush>
              <Table
                caption="Close-job runs"
                columns={[
                  { key: 'id', header: 'Run', cell: (r: CloseRun) => r.id },
                  {
                    key: 'started',
                    header: 'Started',
                    cell: (r) => formatTime(r.startedAt),
                  },
                  {
                    key: 'status',
                    header: 'Status',
                    cell: (r) => <StatusTag status={RUN_STATUS[r.status]}>{r.status}</StatusTag>,
                  },
                  {
                    key: 'windows',
                    header: 'Windows',
                    numeric: true,
                    cell: (r) => r.windows,
                  },
                  { key: 'error', header: 'Error', cell: (r) => r.error ?? '' },
                ]}
                rows={h.closeRuns}
                rowKey={(r) => r.id}
              />
            </Card>
            <Card title="Exporters" flush>
              <Table
                caption="Exporters"
                columns={[
                  {
                    key: 'name',
                    header: 'Exporter',
                    cell: (e: ExporterStatus) => e.name,
                  },
                  {
                    key: 'status',
                    header: 'Status',
                    cell: (e) => <Badge variant={EXPORTER_BADGE[e.status]}>{e.status}</Badge>,
                  },
                  {
                    key: 'cursor',
                    header: 'Cursor',
                    cell: (e) => <code>{e.cursor}</code>,
                  },
                  {
                    key: 'last',
                    header: 'Last success',
                    cell: (e) => (e.lastSuccess ? formatTime(e.lastSuccess) : '—'),
                  },
                  { key: 'error', header: 'Error', cell: (e) => e.error ?? '' },
                ]}
                rows={h.exporters}
                rowKey={(e) => e.name}
              />
            </Card>
          </div>
        );
      }}
    </Loaded>
  );
}

/** One operator section, inside the layout. */
export function OperatorSectionPage({
  section,
  customer,
  client,
  navigate,
  sample,
}: {
  section: OperatorSection;
  customer?: string;
  client: OperatorClient;
  navigate: (to: string) => void;
  sample?: boolean;
}) {
  return (
    <OperatorLayout section={section} navigate={navigate} sample={sample}>
      {section === 'meters' && <MetersPage client={client} />}
      {section === 'plans' && <PlansPage client={client} />}
      {section === 'customers' &&
        (customer ? (
          <CustomerPage id={customer} client={client} navigate={navigate} />
        ) : (
          <CustomersPage client={client} navigate={navigate} />
        ))}
      {section === 'health' && <HealthPage client={client} />}
    </OperatorLayout>
  );
}
