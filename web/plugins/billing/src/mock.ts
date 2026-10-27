// A fixture-backed BillingClient, so the pages work before the meter serves its API (ME1 Task
// 27). It keeps the rules the server will enforce (§13): overage needs a card and a spend cap,
// and only a billing admin changes spend controls. Every page over it shows the sample notice.

import { fromNanos, money, toNanos } from './format.js';
import type {
  AlertsReport,
  BillingClient,
  DailyUsage,
  Delivery,
  EntitlementsReport,
  FeatureUsage,
  PaymentState,
  PaymentView,
  SpendControls,
  SpendControlsUpdate,
  ThresholdRule,
  UsageReport,
  Webhook,
} from './types.js';

/** A refusal with the meter's `ErrorInfo.reason` (§17.4). */
export class BillingError extends Error {
  constructor(
    readonly reason:
      | 'payment_method_required'
      | 'spend_cap_required'
      | 'permission_denied'
      | 'not_found'
      | 'invalid_argument',
    message: string,
  ) {
    super(message);
    this.name = 'BillingError';
  }
}

export interface MockBillingOptions {
  /** The clock; fixtures are laid out in the month around it. */
  now?: () => number;
  paymentState?: PaymentState;
  /** The caller holds `billing_admin`. */
  billingAdmin?: boolean;
  /** Milliseconds every call takes, to show loading states. */
  latencyMs?: number;
}

const DAY = 86_400_000;

function monthOf(now: number) {
  const d = new Date(now);
  const start = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1);
  const end = Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1);
  return { start, end };
}

const iso = (ms: number) => new Date(ms).toISOString();

/** `total` spread over the days from `start` to `now`, unevenly and deterministically. */
function spread(total: bigint, start: number, now: number, seed: number): DailyUsage[] {
  const days = Math.max(1, Math.ceil((now - start) / DAY));
  const weights: bigint[] = [];
  let x = seed;
  for (let i = 0; i < days; i++) {
    x = (x * 1_103_515_245 + 12_345) % 2_147_483_648;
    weights.push(BigInt(50 + (x % 100)));
  }
  const sum = weights.reduce((a, b) => a + b, 0n);
  let given = 0n;
  return weights.map((w, i) => {
    const last = i === days - 1;
    const share = last ? total - given : (total * w) / sum;
    given += share;
    return {
      day: iso(start + i * DAY).slice(0, 10),
      amount: fromNanos(share),
      provisional: last || undefined,
    };
  });
}

function feature(
  base: Omit<FeatureUsage, 'balance' | 'overage' | 'daily' | 'byNamespace' | 'period'>,
  period: { start: number; end: number },
  now: number,
  seed: number,
  namespaces: [string, number][],
): FeatureUsage {
  const used = toNanos(base.usage);
  const total = toNanos(base.included);
  const left = total - used;
  let given = 0n;
  const byNamespace = namespaces.map(([namespace, pct], i) => {
    const share = i === namespaces.length - 1 ? used - given : (used * BigInt(pct)) / 100n;
    given += share;
    return { namespace, usage: fromNanos(share) };
  });
  return {
    ...base,
    period: { start: iso(period.start), end: iso(period.end) },
    balance: fromNanos(left > 0n ? left : 0n),
    overage: fromNanos(left < 0n ? -left : 0n),
    daily: spread(used, period.start, now, seed),
    byNamespace,
  };
}

const wait = (ms: number) =>
  ms > 0 ? new Promise<void>((resolve) => setTimeout(resolve, ms)) : Promise.resolve();

export function createMockBillingClient(opts: MockBillingOptions = {}): BillingClient {
  const now = opts.now ?? Date.now;
  const latency = opts.latencyMs ?? 0;
  const admin = opts.billingAdmin ?? true;
  const paymentState: PaymentState = opts.paymentState ?? 'none';
  let controls: SpendControlsUpdate = {
    overageEnabled: false,
    autoTopUp: { enabled: false, below: money(10), amount: money(50) },
  };
  let rules: ThresholdRule[] = [
    { id: 'rule-default', thresholds: [80, 100], spendCap: true, channels: ['console', 'email'] },
  ];
  let webhooks: Webhook[] = [
    {
      id: 'wh-1',
      url: 'https://hooks.example.com/loams/billing',
      createdAt: iso(now() - 12 * DAY),
      types: ['io.loams.dev.meter.threshold.v1', 'io.loams.dev.meter.access.changed.v1'],
    },
  ];
  const deliveries: Delivery[] = [
    {
      id: 'dl-3',
      webhook: 'wh-1',
      type: 'io.loams.dev.meter.access.changed.v1',
      at: iso(now() - 2 * 3_600_000),
      status: 200,
      attempt: 1,
    },
    {
      id: 'dl-2',
      webhook: 'wh-1',
      type: 'io.loams.dev.meter.threshold.v1',
      at: iso(now() - 26 * 3_600_000),
      status: 503,
      attempt: 3,
    },
    {
      id: 'dl-1',
      webhook: 'wh-1',
      type: 'io.loams.dev.meter.threshold.v1',
      at: iso(now() - 3 * DAY),
      status: 200,
      attempt: 1,
    },
  ];
  let next = 1;
  const seen = new Map<string, unknown>();
  /** Replays a mutation's answer for a repeated idempotency key, as AP0 asks. */
  const once = async <T>(key: string, run: () => T): Promise<T> => {
    await wait(latency);
    if (seen.has(key)) return seen.get(key) as T;
    const out = run();
    seen.set(key, out);
    return out;
  };
  const requireAdmin = () => {
    if (!admin) throw new BillingError('permission_denied', 'Changing this needs billing_admin.');
  };

  const suggestedCap = () => money(40);

  const spend = (): SpendControls => ({
    paymentState,
    ...controls,
    suggestedCap: suggestedCap(),
    canEdit: admin,
  });

  const usage = (): UsageReport => {
    const t = now();
    const period = monthOf(t);
    const overage = controls.overageEnabled && paymentState !== 'none';
    const features: FeatureUsage[] = [
      feature(
        {
          feature: 'house_cpu',
          label: 'House CPU',
          unit: 'cu_ms',
          mode: overage ? 'overage_allowed' : 'hard_limit',
          usage: '30600000',
          included: '36000000',
          hasAccess: true,
        },
        period,
        t,
        7,
        [
          ['acme/analytics', 70],
          ['acme/default', 30],
        ],
      ),
      feature(
        {
          feature: 'llm_output_tokens',
          label: 'LLM output tokens',
          unit: 'tokens',
          mode: overage ? 'overage_allowed' : 'hard_limit',
          usage: overage ? '1180000' : '1000000',
          included: '1000000',
          hasAccess: overage,
          ...(overage && controls.spendCap
            ? {
                overageSpend: money('3.6', controls.spendCap.currency),
                spendCap: controls.spendCap,
              }
            : {}),
        },
        period,
        t,
        13,
        [
          ['acme/agents', 85],
          ['acme/default', 15],
        ],
      ),
      feature(
        {
          feature: 'api_requests',
          label: 'API requests',
          unit: 'requests',
          mode: 'soft_limit',
          usage: '412000',
          included: '1000000',
          hasAccess: true,
        },
        period,
        t,
        29,
        [['acme/default', 100]],
      ),
    ];
    return {
      customer: 'acme',
      paymentState,
      asOf: iso(Math.floor(t / 60_000) * 60_000 - 60_000),
      features,
    };
  };

  return {
    async usage() {
      await wait(latency);
      return usage();
    },
    async entitlements(): Promise<EntitlementsReport> {
      await wait(latency);
      const t = now();
      const period = monthOf(t);
      return {
        plan: { id: 'free', name: 'Free', version: 3 },
        prepaidCredit: money(0),
        entitlements: [
          {
            id: 'ent-house-cpu',
            feature: 'house_cpu',
            label: 'House CPU',
            unit: 'cu_ms',
            kind: {
              type: 'metered',
              mode: 'hard_limit',
              interval: 'month',
              issueAfterReset: '36000000',
            },
            source: { type: 'plan', plan: 'free', version: 3 },
            grants: [
              {
                id: 'grant-house-cpu-included',
                kind: 'included',
                amount: '36000000',
                remaining: '5400000',
                priority: 10,
                effectiveAt: iso(period.start),
                expiresAt: iso(period.end),
                rollover: { max: '0', min: '0' },
                recurring: true,
              },
            ],
          },
          {
            id: 'ent-llm-out',
            feature: 'llm_output_tokens',
            label: 'LLM output tokens',
            unit: 'tokens',
            kind: {
              type: 'metered',
              mode: 'hard_limit',
              interval: 'month',
              issueAfterReset: '1000000',
            },
            source: { type: 'plan', plan: 'free', version: 3 },
            grants: [
              {
                id: 'grant-llm-included',
                kind: 'included',
                amount: '900000',
                remaining: '0',
                priority: 10,
                effectiveAt: iso(period.start),
                expiresAt: iso(period.end),
                rollover: { max: '0', min: '0' },
                recurring: true,
              },
              {
                id: 'grant-llm-promo',
                kind: 'promotional',
                amount: '100000',
                remaining: '0',
                priority: 1,
                effectiveAt: iso(t - 20 * DAY),
                expiresAt: iso(t + 40 * DAY),
                rollover: { max: '100000', min: '0' },
                recurring: false,
              },
            ],
          },
          {
            id: 'ent-api',
            feature: 'api_requests',
            label: 'API requests',
            unit: 'requests',
            kind: {
              type: 'metered',
              mode: 'soft_limit',
              interval: 'month',
              issueAfterReset: '1000000',
            },
            source: { type: 'plan', plan: 'free', version: 3 },
            grants: [],
          },
          {
            id: 'ent-branches',
            feature: 'pg_branches',
            label: 'Postgres branches',
            unit: 'count',
            kind: { type: 'static', config: { max: 10 } },
            source: { type: 'plan', plan: 'free', version: 3 },
            grants: [],
          },
          {
            id: 'ent-byoc',
            feature: 'byoc',
            label: 'Bring your own cloud',
            unit: 'count',
            kind: { type: 'boolean', enabled: false },
            source: { type: 'plan', plan: 'free', version: 3 },
            grants: [],
          },
        ],
      };
    },
    async spendControls() {
      await wait(latency);
      return spend();
    },
    setSpendControls(update, key) {
      return once(key, () => {
        requireAdmin();
        if (update.overageEnabled && paymentState === 'none') {
          throw new BillingError(
            'payment_method_required',
            'Overage needs a payment method: add a card first.',
          );
        }
        if (update.overageEnabled && !update.spendCap) {
          throw new BillingError('spend_cap_required', 'Overage needs a spend cap.');
        }
        if (update.spendCap && BigInt(update.spendCap.micros) <= 0n) {
          throw new BillingError('invalid_argument', 'The spend cap must be more than zero.');
        }
        controls = { ...update };
        return spend();
      });
    },
    async payment(): Promise<PaymentView> {
      await wait(latency);
      return {
        state: paymentState,
        provider: 'stripe',
        portalUrl: 'https://billing.stripe.com/p/session/test_sample',
        methodSummary: paymentState === 'card' ? 'Visa ending 4242' : undefined,
        canEdit: admin,
      };
    },
    async alerts(): Promise<AlertsReport> {
      await wait(latency);
      return { rules, webhooks, deliveries, canEdit: admin };
    },
    createRule(rule, key) {
      return once(key, () => {
        requireAdmin();
        if (rule.thresholds.length === 0 || rule.thresholds.some((p) => p <= 0 || p > 1000)) {
          throw new BillingError('invalid_argument', 'Thresholds are percentages above zero.');
        }
        const created = { ...rule, id: `rule-${next++}` };
        rules = [...rules, created];
        return created;
      });
    },
    async deleteRule(id) {
      await wait(latency);
      requireAdmin();
      if (!rules.some((r) => r.id === id)) throw new BillingError('not_found', 'No such rule.');
      rules = rules.filter((r) => r.id !== id);
    },
    createWebhook(url, key) {
      return once(key, () => {
        requireAdmin();
        if (!/^https:\/\/[^\s]+$/.test(url)) {
          throw new BillingError('invalid_argument', 'A webhook URL must be https.');
        }
        const webhook: Webhook = {
          id: `wh-${++next}`,
          url,
          createdAt: iso(now()),
          types: ['io.loams.dev.meter.threshold.v1', 'io.loams.dev.meter.access.changed.v1'],
        };
        webhooks = [...webhooks, webhook];
        return { webhook, secret: `whsec_sample${next}` };
      });
    },
    async rotateWebhookSecret(id) {
      await wait(latency);
      requireAdmin();
      if (!webhooks.some((w) => w.id === id))
        throw new BillingError('not_found', 'No such webhook.');
      return { secret: `whsec_sample${++next}` };
    },
  };
}
