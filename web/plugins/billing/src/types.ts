// The shapes the billing pages read, as the meter's REST mapping (`/v1/meter/…`, design 63
// §17.2) will answer them. They mirror `loams-meter-core` (ME1 Task 1) in JSON:
//
// - an Amount is a decimal string with up to 9 decimals ("12", "0.5"), never a float;
// - Money is integer micros (int64, a string in proto JSON) plus an ISO 4217 code (D374);
// - times are RFC 3339 strings.
//
// Until ME-svc serves those methods the pages run on the fixture client (mock.ts); see
// .he0-shared/me1-notes.md, "ME-ui", for what the console expects of each method.

/** A fixed-point decimal, as `Amount`'s serde form: "12", "0.5", "-3.000000001". */
export type Amount = string;

export interface Money {
  /** Integer micros, as a decimal string (int64 in proto JSON). */
  micros: string;
  /** ISO 4217, upper case. */
  currency: string;
}

/** `Unit` in loams-meter-core, snake case; anything else is shown as given. */
export type Unit =
  | 'count'
  | 'ms'
  | 'cu_ms'
  | 'bytes'
  | 'byte_hours'
  | 'tokens'
  | 'requests'
  | (string & {});

/** `Mode`: what happens when the balance reaches zero. */
export type Mode = 'hard_limit' | 'soft_limit' | 'overage_allowed';

/** A customer's payment state (§13), set by the billing exporter or an operator. */
export type PaymentState = 'none' | 'card' | 'invoice';

/** `[start, end)`. */
export interface Period {
  start: string;
  end: string;
}

/** One closed (or, for today, provisional) day of a feature's usage, from `_meter.windows`. */
export interface DailyUsage {
  /** The day, `YYYY-MM-DD`, in UTC. */
  day: string;
  amount: Amount;
  /** The open window: counted so far, not billed yet. */
  provisional?: boolean;
}

/** A metered feature's usage in the current period: the Usage page's row. */
export interface FeatureUsage {
  feature: string;
  label: string;
  unit: Unit;
  mode: Mode;
  period: Period;
  /** Used this period. */
  usage: Amount;
  /** The period's total available (`total_available`): included plus granted credit. */
  included: Amount;
  balance: Amount;
  overage: Amount;
  hasAccess: boolean;
  /** Overage spend so far and its cap, for `overage_allowed`. */
  overageSpend?: Money;
  spendCap?: Money;
  /** Prepaid credit only: no included allowance, stops at zero. */
  prepaidOnly?: boolean;
  daily: DailyUsage[];
  byNamespace: { namespace: string; usage: Amount }[];
}

export interface UsageReport {
  customer: string;
  paymentState: PaymentState;
  /** Everything up to here is closed in the House; later counts are provisional. */
  asOf: string;
  features: FeatureUsage[];
}

export type GrantKind = 'promotional' | 'included' | 'prepaid';

export interface Grant {
  id: string;
  kind: GrantKind;
  amount: Amount;
  /** What is left of it this period. */
  remaining: Amount;
  priority: number;
  effectiveAt: string;
  /** None: does not expire. */
  expiresAt?: string;
  rollover: { max: Amount; min: Amount };
  /** Recurs every period (a plan's included grant). */
  recurring: boolean;
  /** What a prepaid grant cost. */
  paid?: Money;
}

export type EntitlementKind =
  | { type: 'metered'; mode: Mode; interval: string; issueAfterReset?: Amount }
  | { type: 'boolean'; enabled: boolean }
  | { type: 'static'; config: Record<string, unknown> };

export interface EntitlementView {
  id: string;
  feature: string;
  label: string;
  unit: Unit;
  kind: EntitlementKind;
  source: { type: 'plan'; plan: string; version: number } | { type: 'manual'; by: string };
  grants: Grant[];
}

export interface EntitlementsReport {
  plan?: { id: string; name: string; version: number };
  entitlements: EntitlementView[];
  /** Prepaid credit left across features, when the customer bought some. */
  prepaidCredit?: Money;
}

export interface SpendControls {
  paymentState: PaymentState;
  overageEnabled: boolean;
  /** Required while overage is on (§13); the default is twice the plan's included value (Q961). */
  spendCap?: Money;
  suggestedCap: Money;
  autoTopUp?: { enabled: boolean; below: Money; amount: Money };
  /** The caller holds `billing_admin`. */
  canEdit: boolean;
}

export interface SpendControlsUpdate {
  overageEnabled: boolean;
  spendCap?: Money;
  autoTopUp?: { enabled: boolean; below: Money; amount: Money };
}

export interface PaymentView {
  state: PaymentState;
  /** The billing provider, when one is configured ("stripe"). */
  provider?: string;
  /** The provider's hosted page; Loams never handles card data. */
  portalUrl?: string;
  /** A short description the provider gave ("Visa ending 4242"); never card data itself. */
  methodSummary?: string;
  canEdit: boolean;
}

export interface ThresholdRule {
  id: string;
  /** The feature, or every feature. */
  feature?: string;
  /** Percentages of the period's total available (default 80 and 100). */
  thresholds: number[];
  /** Also of the spend cap. */
  spendCap: boolean;
  channels: ('console' | 'email' | 'webhook')[];
}

export interface Webhook {
  id: string;
  url: string;
  createdAt: string;
  /** Types it receives, `io.loams.dev.meter.*`. */
  types: string[];
}

export interface Delivery {
  id: string;
  webhook: string;
  type: string;
  at: string;
  /** HTTP status, or 0 when the endpoint could not be reached. */
  status: number;
  attempt: number;
}

export interface AlertsReport {
  rules: ThresholdRule[];
  webhooks: Webhook[];
  deliveries: Delivery[];
  canEdit: boolean;
}

/** The customer-facing client: Settings › Billing. */
export interface BillingClient {
  usage(): Promise<UsageReport>;
  entitlements(): Promise<EntitlementsReport>;
  spendControls(): Promise<SpendControls>;
  /** `CustomerService/SetSpendControls`; audited. */
  setSpendControls(update: SpendControlsUpdate, idempotencyKey: string): Promise<SpendControls>;
  payment(): Promise<PaymentView>;
  alerts(): Promise<AlertsReport>;
  createRule(rule: Omit<ThresholdRule, 'id'>, idempotencyKey: string): Promise<ThresholdRule>;
  deleteRule(id: string): Promise<void>;
  /** The secret is shown once. */
  createWebhook(url: string, idempotencyKey: string): Promise<{ webhook: Webhook; secret: string }>;
  rotateWebhookSecret(id: string): Promise<{ secret: string }>;
}
