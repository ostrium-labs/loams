// The operator view of Loams Meter (design 63 §18, ME1 Task 28): Meters, Plans, Customers and
// Meter health. Same JSON rules as ../types.ts (amounts as decimal strings, money as micros).
// Reads need `meter_admin`; manual grants are written with a reason and audited (§17.1).

import type { Amount, EntitlementView, Money, PaymentState, Unit } from '../types.js';

/** loams-meter-core `Aggregation`, OpenMeter's names. */
export type Aggregation = 'SUM' | 'COUNT' | 'UNIQUE_COUNT' | 'AVG' | 'MIN' | 'MAX' | 'LATEST';

export interface MeterDef {
  slug: string;
  description?: string;
  /** The CloudEvents `type` it counts. */
  eventType: string;
  aggregation: Aggregation;
  /** A JSONPath into `data`; none for COUNT. */
  valueProperty?: string;
  groupBy: Record<string, string>;
  /** Events before this are not counted. */
  eventFrom?: string;
  createdAt: string;
  archived: boolean;
  /** Once events were counted, the semantics are immutable (D1137, Stripe's rule). */
  inUse: boolean;
  /** Events counted in the last 24 hours, for a quick sanity check. */
  events24h?: number;
}

export interface PlanTemplate {
  feature: string;
  unit: Unit;
  kind:
    | {
        type: 'metered';
        mode: string;
        interval: string;
        issueAfterReset?: Amount;
        overagePrice?: Money;
      }
    | { type: 'boolean'; enabled: boolean }
    | { type: 'static'; config: Record<string, unknown> };
}

export interface PlanVersion {
  version: number;
  appliedAt: string;
  /** Who applied it: the GitOps reconciler or an operator (§22). */
  appliedBy: string;
  entitlements: PlanTemplate[];
  customers: number;
}

export interface PlanDef {
  id: string;
  name: string;
  /** Newest first. */
  versions: PlanVersion[];
}

export interface CustomerSummary {
  id: string;
  name: string;
  subjects: string[];
  paymentState: PaymentState;
  plan?: { id: string; version: number };
  /** Provider ids, by provider ("stripe": "cus_…"). */
  providerIds: Record<string, string>;
  /** Features at a hard stop right now. */
  stopped: string[];
}

export interface CustomerBalance {
  feature: string;
  label: string;
  unit: Unit;
  usage: Amount;
  balance: Amount;
  overage: Amount;
  hasAccess: boolean;
}

export interface CustomerDetail extends CustomerSummary {
  entitlements: EntitlementView[];
  balances: CustomerBalance[];
  spendCap?: Money;
  overageEnabled: boolean;
}

export interface ManualGrant {
  entitlement: string;
  amount: Amount;
  kind: 'promotional' | 'prepaid';
  /** Days until it expires; none: never. */
  expiresInDays?: number;
  /** Required and audited. */
  reason: string;
}

export interface PartitionOwner {
  partition: number;
  /** The node holding the partition lease, or none while it is being taken over. */
  owner?: string;
  epoch: number;
  /** Leases outstanding on gates. */
  leases: number;
  committedOffset: number;
  /** Seconds since the owner rebuilt, when it did recently. */
  rebuildSeconds?: number;
}

export interface DriftRow {
  subject: string;
  feature: string;
  /** (counter − House) / House, as a ratio. */
  drift: number;
  checkedAt: string;
}

export interface CloseRun {
  id: string;
  startedAt: string;
  finishedAt?: string;
  status: 'running' | 'ok' | 'failed';
  windows: number;
  error?: string;
}

export interface ExporterStatus {
  name: string;
  /** The last window or offset exported. */
  cursor: string;
  lastSuccess?: string;
  status: 'ok' | 'retrying' | 'failed' | 'off';
  error?: string;
}

export interface MeterHealth {
  asOf: string;
  partitions: PartitionOwner[];
  drift: DriftRow[];
  /** `loams_house_pipe_lag_records{pipe="_meter"}`. */
  pipeLagRecords: number;
  pipeLagSeconds: number;
  closeRuns: CloseRun[];
  exporters: ExporterStatus[];
  /** `loams_events_dropped_total{stream="_meter"}` over the last 24 hours. */
  droppedEvents24h: number;
  /** `loams_meter_hard_limit_overrun_ratio`, the worst subject. */
  hardLimitOverrun: number;
}

export interface OperatorClient {
  meters(): Promise<MeterDef[]>;
  plans(): Promise<PlanDef[]>;
  customers(query: string): Promise<CustomerSummary[]>;
  customer(id: string): Promise<CustomerDetail>;
  /** `EntitlementService/CreateGrant`; the reason is audited. */
  createGrant(customer: string, grant: ManualGrant, idempotencyKey: string): Promise<void>;
  health(): Promise<MeterHealth>;
}
