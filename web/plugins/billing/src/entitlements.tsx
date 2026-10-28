// Settings › Billing › Entitlements (design 63 §18, §9): the plan, each entitlement and its
// mode, its grants with expiry and rollover in burn-down order, and prepaid credit.

import { Badge, Card, Primary, Stat, Stats, Table } from '@loams/ui';
import { formatAmount, formatDay, formatMoney, toNanos } from './format.js';
import { Loaded, useLoad } from './load.js';
import type { BillingClient, EntitlementView, Grant, GrantKind } from './types.js';
import { MODE_LABEL } from './usage.js';

const KIND_LABEL: Record<GrantKind, string> = {
  promotional: 'Promotional',
  included: 'Included',
  prepaid: 'Prepaid',
};
const KIND_ORDER: Record<GrantKind, number> = { promotional: 0, included: 1, prepaid: 2 };

/**
 * Grants in the order usage burns them (loams-meter-core `Grant::burn_order_key`): priority,
 * nearest expiry, promotional before included before prepaid, oldest first, then id.
 */
export function burnOrder(grants: readonly Grant[]): Grant[] {
  const expiry = (g: Grant) => (g.expiresAt ? Date.parse(g.expiresAt) : Number.POSITIVE_INFINITY);
  return [...grants].sort(
    (a, b) =>
      a.priority - b.priority ||
      expiry(a) - expiry(b) ||
      KIND_ORDER[a.kind] - KIND_ORDER[b.kind] ||
      Date.parse(a.effectiveAt) - Date.parse(b.effectiveAt) ||
      a.id.localeCompare(b.id),
  );
}

function rolloverText(g: Grant, e: EntitlementView): string {
  const max = toNanos(g.rollover.max);
  if (max === 0n) return 'None';
  return `Up to ${formatAmount(g.rollover.max, e.unit)}`;
}

function kindText(e: EntitlementView): string {
  switch (e.kind.type) {
    case 'metered':
      return `${MODE_LABEL[e.kind.mode]}, per ${e.kind.interval}${
        e.kind.issueAfterReset
          ? `, ${formatAmount(e.kind.issueAfterReset, e.unit)} each period`
          : ''
      }`;
    case 'boolean':
      return e.kind.enabled ? 'Included' : 'Not included';
    case 'static':
      return Object.entries(e.kind.config)
        .map(([k, v]) => `${k}: ${String(v)}`)
        .join(', ');
  }
}

export function EntitlementCard({ e }: { e: EntitlementView }) {
  return (
    <Card
      title={e.label}
      actions={
        <span className="flex items-center gap-2">
          <Badge variant="outline">{e.kind.type}</Badge>
          <Badge variant="neutral">
            {e.source.type === 'plan'
              ? `Plan ${e.source.plan} v${e.source.version}`
              : `Manual, by ${e.source.by}`}
          </Badge>
        </span>
      }
    >
      <p className="mb-3 text-muted">{kindText(e)}</p>
      {e.kind.type === 'metered' && (
        <Table
          caption={`${e.label} grants, in burn-down order`}
          columns={[
            {
              key: 'grant',
              header: 'Grant',
              cell: (g: Grant) => (
                <Primary
                  title={KIND_LABEL[g.kind]}
                  detail={`${g.recurring ? 'Every period' : 'Once'}${
                    g.paid ? `, paid ${formatMoney(g.paid)}` : ''
                  }`}
                />
              ),
            },
            {
              key: 'amount',
              header: 'Amount',
              numeric: true,
              cell: (g) => formatAmount(g.amount, e.unit),
            },
            {
              key: 'left',
              header: 'Remaining',
              numeric: true,
              cell: (g) => formatAmount(g.remaining, e.unit),
            },
            { key: 'priority', header: 'Priority', numeric: true, cell: (g) => g.priority },
            {
              key: 'expires',
              header: 'Expires',
              cell: (g) => (g.expiresAt ? formatDay(g.expiresAt) : 'Never'),
            },
            { key: 'rollover', header: 'Rollover', cell: (g) => rolloverText(g, e) },
          ]}
          rows={burnOrder(e.grants)}
          rowKey={(g) => g.id}
          empty={<p className="text-muted">No grants: usage counts against the plan only.</p>}
        />
      )}
    </Card>
  );
}

export function EntitlementsPage({ client }: { client: BillingClient }) {
  const [load] = useLoad(() => client.entitlements(), [client]);
  return (
    <Loaded load={load} what="entitlements">
      {(report) => (
        <div className="grid gap-4">
          <Stats>
            <Stat
              label="Plan"
              value={report.plan ? report.plan.name : 'No plan'}
              detail={report.plan ? `Version ${report.plan.version}` : undefined}
            />
            <Stat
              label="Prepaid credit"
              value={report.prepaidCredit ? formatMoney(report.prepaidCredit) : '—'}
            />
          </Stats>
          {report.entitlements.map((e) => (
            <EntitlementCard key={e.id} e={e} />
          ))}
        </div>
      )}
    </Loaded>
  );
}
