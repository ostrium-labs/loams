// A fixture OperatorClient (ME1 Task 28), so the operator pages work before the meter serves
// loams.meter.v1. It keeps the server's rules for manual grants: a reason, a positive amount and
// a metered entitlement of that customer.

import { addAmounts, money, toNanos } from '../format.js';
import { BillingError } from '../mock.js';
import type { EntitlementView } from '../types.js';
import type {
  CustomerDetail,
  CustomerSummary,
  MeterDef,
  MeterHealth,
  OperatorClient,
  PlanDef,
} from './types.js';

export interface MockOperatorOptions {
  now?: () => number;
  latencyMs?: number;
}

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const wait = (ms: number) =>
  ms > 0 ? new Promise<void>((resolve) => setTimeout(resolve, ms)) : Promise.resolve();

const METERS: MeterDef[] = [
  {
    slug: 'house_cpu',
    description: 'House query CPU time',
    eventType: 'io.loams.dev.usage.house.query.v1',
    aggregation: 'SUM',
    valueProperty: '$.cpu_ms',
    groupBy: { namespace: '$.namespace' },
    createdAt: '2026-10-19T09:00:00Z',
    archived: false,
    inUse: true,
    events24h: 182_340,
  },
  {
    slug: 'llm_output_tokens',
    eventType: 'io.loams.dev.usage.llm.tokens.v1',
    aggregation: 'SUM',
    valueProperty: '$.output_tokens',
    groupBy: { model: '$.model', namespace: '$.namespace' },
    createdAt: '2026-10-19T09:00:00Z',
    archived: false,
    inUse: true,
    events24h: 48_210,
  },
  {
    slug: 'api_requests',
    eventType: 'io.loams.dev.usage.gateway.request.v1',
    aggregation: 'COUNT',
    groupBy: { route: '$.route' },
    createdAt: '2026-10-20T12:00:00Z',
    archived: false,
    inUse: true,
    events24h: 1_204_552,
  },
  {
    slug: 'pg_active_users',
    eventType: 'io.loams.dev.usage.pg.session.v1',
    aggregation: 'UNIQUE_COUNT',
    valueProperty: '$.user',
    groupBy: {},
    createdAt: '2026-10-21T08:00:00Z',
    archived: true,
    inUse: false,
  },
];

const PLANS: PlanDef[] = [
  {
    id: 'free',
    name: 'Free',
    versions: [
      {
        version: 3,
        appliedAt: '2026-10-22T10:00:00Z',
        appliedBy: 'gitops',
        customers: 41,
        entitlements: [
          {
            feature: 'house_cpu',
            unit: 'cu_ms',
            kind: {
              type: 'metered',
              mode: 'hard_limit',
              interval: 'month',
              issueAfterReset: '36000000',
            },
          },
          {
            feature: 'llm_output_tokens',
            unit: 'tokens',
            kind: {
              type: 'metered',
              mode: 'hard_limit',
              interval: 'month',
              issueAfterReset: '1000000',
            },
          },
          {
            feature: 'pg_branches',
            unit: 'count',
            kind: { type: 'static', config: { max: 10 } },
          },
          {
            feature: 'byoc',
            unit: 'count',
            kind: { type: 'boolean', enabled: false },
          },
        ],
      },
      {
        version: 2,
        appliedAt: '2026-10-19T10:00:00Z',
        appliedBy: 'gitops',
        customers: 0,
        entitlements: [
          {
            feature: 'house_cpu',
            unit: 'cu_ms',
            kind: {
              type: 'metered',
              mode: 'hard_limit',
              interval: 'month',
              issueAfterReset: '18000000',
            },
          },
        ],
      },
    ],
  },
  {
    id: 'team',
    name: 'Team',
    versions: [
      {
        version: 1,
        appliedAt: '2026-10-22T10:00:00Z',
        appliedBy: 'gitops',
        customers: 7,
        entitlements: [
          {
            feature: 'house_cpu',
            unit: 'cu_ms',
            kind: {
              type: 'metered',
              mode: 'overage_allowed',
              interval: 'month',
              issueAfterReset: '360000000',
              overagePrice: money('0.12'),
            },
          },
          {
            feature: 'byoc',
            unit: 'count',
            kind: { type: 'boolean', enabled: true },
          },
        ],
      },
    ],
  },
];

function entitlementsFor(plan: string): EntitlementView[] {
  const version = PLANS.find((p) => p.id === plan)?.versions[0];
  return (version?.entitlements ?? []).map((t) => ({
    id: `ent-${plan}-${t.feature}`,
    feature: t.feature,
    label: t.feature,
    unit: t.unit,
    kind:
      t.kind.type === 'metered'
        ? {
            type: 'metered',
            mode: t.kind.mode as 'hard_limit' | 'soft_limit' | 'overage_allowed',
            interval: t.kind.interval,
            issueAfterReset: t.kind.issueAfterReset,
          }
        : t.kind,
    source: { type: 'plan', plan, version: version?.version ?? 0 },
    grants: [],
  }));
}

export function createMockOperatorClient(opts: MockOperatorOptions = {}): OperatorClient {
  const now = opts.now ?? Date.now;
  const latency = opts.latencyMs ?? 0;
  const customers: CustomerDetail[] = [
    {
      id: 'cus-acme',
      name: 'Acme',
      subjects: ['acme'],
      paymentState: 'none',
      plan: { id: 'free', version: 3 },
      providerIds: { stripe: 'cus_sample_acme' },
      stopped: ['llm_output_tokens'],
      entitlements: entitlementsFor('free'),
      balances: [
        {
          feature: 'house_cpu',
          label: 'House CPU',
          unit: 'cu_ms',
          usage: '30600000',
          balance: '5400000',
          overage: '0',
          hasAccess: true,
        },
        {
          feature: 'llm_output_tokens',
          label: 'LLM output tokens',
          unit: 'tokens',
          usage: '1000000',
          balance: '0',
          overage: '0',
          hasAccess: false,
        },
      ],
      overageEnabled: false,
    },
    {
      id: 'cus-globex',
      name: 'Globex',
      subjects: ['globex', 'globex-eu'],
      paymentState: 'card',
      plan: { id: 'team', version: 1 },
      providerIds: { stripe: 'cus_sample_globex' },
      stopped: [],
      entitlements: entitlementsFor('team'),
      balances: [
        {
          feature: 'house_cpu',
          label: 'House CPU',
          unit: 'cu_ms',
          usage: '372000000',
          balance: '0',
          overage: '12000000',
          hasAccess: true,
        },
      ],
      spendCap: money(200),
      overageEnabled: true,
    },
    {
      id: 'cus-initech',
      name: 'Initech',
      subjects: ['initech'],
      paymentState: 'invoice',
      plan: { id: 'team', version: 1 },
      providerIds: {},
      stopped: [],
      entitlements: entitlementsFor('team'),
      balances: [],
      overageEnabled: true,
    },
  ];
  const seen = new Set<string>();
  const summary = (c: CustomerDetail): CustomerSummary => ({
    id: c.id,
    name: c.name,
    subjects: [...c.subjects],
    paymentState: c.paymentState,
    plan: c.plan,
    providerIds: { ...c.providerIds },
    stopped: [...c.stopped],
  });

  return {
    async meters() {
      await wait(latency);
      return METERS;
    },
    async plans() {
      await wait(latency);
      return PLANS;
    },
    async customers(query) {
      await wait(latency);
      const q = query.trim().toLowerCase();
      return customers
        .filter(
          (c) =>
            !q ||
            c.name.toLowerCase().includes(q) ||
            c.id.includes(q) ||
            c.subjects.some((s) => s.includes(q)) ||
            Object.values(c.providerIds).some((id) => id.includes(q)),
        )
        .map(summary);
    },
    async customer(id) {
      await wait(latency);
      const c = customers.find((x) => x.id === id);
      if (!c) throw new BillingError('not_found', `No customer ${id}.`);
      return structuredClone(c);
    },
    async createGrant(customerId, grant, key) {
      await wait(latency);
      if (seen.has(key)) return;
      const c = customers.find((x) => x.id === customerId);
      if (!c) throw new BillingError('not_found', `No customer ${customerId}.`);
      const e = c.entitlements.find((x) => x.id === grant.entitlement);
      if (!e || e.kind.type !== 'metered') {
        throw new BillingError('invalid_argument', 'Grants go on a metered entitlement.');
      }
      if (!grant.reason.trim())
        throw new BillingError('invalid_argument', 'A grant needs a reason.');
      if (toNanos(grant.amount) <= 0n) {
        throw new BillingError('invalid_argument', 'The amount must be more than zero.');
      }
      seen.add(key);
      const t = now();
      e.grants.push({
        id: `grant-manual-${seen.size}`,
        kind: grant.kind,
        amount: grant.amount,
        remaining: grant.amount,
        priority: 5,
        effectiveAt: new Date(t).toISOString(),
        expiresAt: grant.expiresInDays
          ? new Date(t + grant.expiresInDays * DAY).toISOString()
          : undefined,
        rollover: { max: '0', min: '0' },
        recurring: false,
      });
      const b = c.balances.find((x) => x.feature === e.feature);
      if (b) {
        b.balance = addAmounts(b.balance, grant.amount);
        if (toNanos(b.balance) > 0n && !b.hasAccess) {
          b.hasAccess = true;
          c.stopped = c.stopped.filter((f) => f !== e.feature);
        }
      }
    },
    async health(): Promise<MeterHealth> {
      await wait(latency);
      const t = now();
      const at = (ms: number) => new Date(t - ms).toISOString();
      return {
        asOf: at(0),
        partitions: Array.from({ length: 8 }, (_, i) => ({
          partition: i,
          owner: i === 5 ? undefined : `loams-${i % 3}`,
          epoch: i === 5 ? 4 : 3,
          leases: i === 5 ? 0 : 3 + ((i * 7) % 11),
          committedOffset: 120_000 + i * 9_731,
          rebuildSeconds: i === 2 ? 4.2 : undefined,
        })),
        drift: [
          {
            subject: 'acme',
            feature: 'house_cpu',
            drift: 0.002,
            checkedAt: at(3 * 60_000),
          },
          {
            subject: 'globex',
            feature: 'house_cpu',
            drift: 0.014,
            checkedAt: at(3 * 60_000),
          },
          {
            subject: 'globex',
            feature: 'llm_output_tokens',
            drift: -0.0004,
            checkedAt: at(3 * 60_000),
          },
        ],
        pipeLagRecords: 1_240,
        pipeLagSeconds: 38,
        closeRuns: [
          {
            id: 'run-312',
            startedAt: at(40_000),
            status: 'running',
            windows: 0,
          },
          {
            id: 'run-311',
            startedAt: at(HOUR + 40_000),
            finishedAt: at(HOUR),
            status: 'ok',
            windows: 1_820,
          },
          {
            id: 'run-310',
            startedAt: at(2 * HOUR + 40_000),
            finishedAt: at(2 * HOUR),
            status: 'failed',
            windows: 0,
            error: 'House unavailable: connection refused',
          },
        ],
        exporters: [
          {
            name: 'stripe',
            cursor: at(HOUR).slice(0, 13),
            lastSuccess: at(50 * 60_000),
            status: 'ok',
          },
          {
            name: 'openmeter',
            cursor: 'offset 118204',
            lastSuccess: at(7 * HOUR),
            status: 'retrying',
            error: 'HTTP 503 from https://openmeter.example.com',
          },
          { name: 'lago', cursor: '—', status: 'off' },
        ],
        droppedEvents24h: 0,
        hardLimitOverrun: 0.003,
      };
    },
  };
}
