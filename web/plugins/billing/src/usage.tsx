// Settings › Billing › Usage (design 63 §18): per feature, what was used, what is included, the
// balance and any overage this period, a daily chart from `_meter.windows`, and the namespaces
// it came from. Everything after `as_of` is the open window: counted, provisional, not billed.

import { type ChartSpec, LiveChart } from '@loams/charts';
import { Badge, Card, Meter, Stat, Stats, Table } from '@loams/ui';
import {
  formatAmount,
  formatDay,
  formatMoney,
  formatPercent,
  formatTime,
  ratio,
  toNanos,
  unitLabel,
} from './format.js';
import { Loaded, useLoad } from './load.js';
import type { BillingClient, FeatureUsage, Mode, PaymentState } from './types.js';

export const MODE_LABEL: Record<Mode, string> = {
  hard_limit: 'Hard limit',
  soft_limit: 'Soft limit',
  overage_allowed: 'Overage allowed',
};

export const PAYMENT_LABEL: Record<PaymentState, string> = {
  none: 'No payment method',
  card: 'Card on file',
  invoice: 'Invoiced (contract)',
};

const DAILY_SPEC = (f: FeatureUsage): ChartSpec => ({
  chartType: 'Bar Chart',
  title: `${f.label} per day`,
  encodings: {
    x: { field: 'day', type: 'temporal' },
    y: { field: 'value', type: 'quantitative' },
    color: { field: 'state', type: 'nominal' },
  },
  chartProperties: { unit: unitLabel(f.unit) },
});

/** The chart's rows: one per day, in the unit a person reads, closed or provisional. */
export function dailyRows(f: FeatureUsage) {
  const divisor = f.unit === 'ms' || f.unit === 'cu_ms' ? 3_600_000 : f.unit === 'bytes' ? 1e9 : 1;
  return f.daily.map((d) => ({
    day: Date.parse(`${d.day}T00:00:00Z`),
    // Display only: a bar's height, never a billed amount.
    value: Number(toNanos(d.amount) / 1_000_000n) / 1000 / divisor,
    state: d.provisional ? 'Provisional' : 'Closed',
  }));
}

function FeatureCard({ f }: { f: FeatureUsage }) {
  const used = ratio(f.usage, f.included);
  const over = toNanos(f.overage) > 0n;
  return (
    <Card
      title={f.label}
      actions={
        <span className="flex items-center gap-2">
          <Badge variant="outline">{MODE_LABEL[f.mode]}</Badge>
          {!f.hasAccess && <Badge variant="danger">Stopped</Badge>}
          {f.hasAccess && used >= 0.8 && used < 1 && <Badge variant="warning">80%</Badge>}
        </span>
      }
    >
      <div className="grid gap-4">
        <div>
          <div className="mb-1 flex justify-between text-small text-muted">
            <span>
              {formatAmount(f.usage, f.unit)} of {formatAmount(f.included, f.unit)}
            </span>
            <span>{formatPercent(used)}</span>
          </div>
          <Meter value={used} max={1} label={`${f.label}: ${formatPercent(used)} used`} />
        </div>
        <Stats>
          <Stat label="Used" value={formatAmount(f.usage, f.unit)} />
          <Stat
            label={f.prepaidOnly ? 'Credit' : 'Included'}
            value={formatAmount(f.included, f.unit)}
          />
          <Stat label="Balance" value={formatAmount(f.balance, f.unit)} />
          <Stat
            label="Overage"
            value={formatAmount(f.overage, f.unit)}
            detail={
              f.overageSpend
                ? `${formatMoney(f.overageSpend)}${f.spendCap ? ` of ${formatMoney(f.spendCap)} cap` : ''}`
                : over
                  ? 'Billed per unit'
                  : undefined
            }
          />
        </Stats>
        <LiveChart
          spec={DAILY_SPEC(f)}
          data={dailyRows(f)}
          height={160}
          ariaLabel={`${f.label} per day, in ${unitLabel(f.unit)}; today is provisional`}
        />
        <Table
          caption={`${f.label} by namespace`}
          columns={[
            { key: 'ns', header: 'Namespace', cell: (r) => r.namespace },
            {
              key: 'used',
              header: 'Used',
              numeric: true,
              cell: (r) => formatAmount(r.usage, f.unit),
            },
            {
              key: 'share',
              header: 'Share',
              numeric: true,
              cell: (r) => formatPercent(ratio(r.usage, f.usage)),
            },
          ]}
          rows={f.byNamespace}
          rowKey={(r) => r.namespace}
        />
      </div>
    </Card>
  );
}

export function UsagePage({ client }: { client: BillingClient }) {
  const [load] = useLoad(() => client.usage(), [client]);
  return (
    <Loaded load={load} what="usage">
      {(report) => {
        const period = report.features[0]?.period;
        return (
          <div className="grid gap-4">
            <Stats>
              <Stat
                label="Period"
                value={period ? `${formatDay(period.start)} – ${formatDay(period.end)}` : '—'}
                detail={period ? `Resets ${formatDay(period.end)}` : undefined}
              />
              <Stat label="Payment" value={PAYMENT_LABEL[report.paymentState]} />
              <Stat
                label="As of"
                value={formatTime(report.asOf)}
                detail="Later usage is provisional until its window closes"
              />
            </Stats>
            {report.features.length === 0 ? (
              <p className="text-muted">No metered features on this plan.</p>
            ) : (
              report.features.map((f) => <FeatureCard key={f.feature} f={f} />)
            )}
          </div>
        );
      }}
    </Loaded>
  );
}
