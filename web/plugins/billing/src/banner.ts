// What the shell's billing banner says (design 63 §18, §13): at 80% of a period's allowance a
// warning; at 100% with a hard stop, what stopped and how to resume (add a card, buy credit,
// raise the cap, or wait for the reset date). Pure, so the rules are tested without a page.

import { formatDay, moneyRatio, ratio } from './format.js';
import type { FeatureUsage, UsageReport } from './types.js';

export type BannerLevel = 'stop' | 'warn' | 'info';

export interface BannerAction {
  label: string;
  /** A console route. */
  href: string;
}

export interface BannerNotice {
  level: BannerLevel;
  feature: string;
  /** The threshold crossed, in percent (80, 100), or 20 for "credits at 20%". */
  threshold: number;
  message: string;
  actions: BannerAction[];
}

/** The billing pages' routes (Settings › Billing, D1080). */
export const BILLING_ROUTES = {
  base: '/settings/billing',
  usage: '/settings/billing/usage',
  entitlements: '/settings/billing/entitlements',
  spend: '/settings/billing/spend',
  payment: '/settings/billing/payment',
  alerts: '/settings/billing/alerts',
} as const;

const RANK: Record<BannerLevel, number> = { stop: 0, warn: 1, info: 2 };

/** "this month's" for a calendar-month period, else "this period's". */
function periodWord(f: FeatureUsage): string {
  const days = (Date.parse(f.period.end) - Date.parse(f.period.start)) / 86_400_000;
  if (days >= 28 && days <= 31) return "this month's";
  if (days === 7) return "this week's";
  if (days === 1) return "today's";
  return "this period's";
}

function noticesFor(f: FeatureUsage, report: UsageReport): BannerNotice[] {
  const reset = formatDay(f.period.end);
  const wait: BannerAction = {
    label: `Wait for the reset on ${reset}`,
    href: BILLING_ROUTES.usage,
  };
  const out: BannerNotice[] = [];

  // The spend cap: overage stops there (§13), and warns at 80% of it.
  if (f.overageSpend && f.spendCap) {
    const r = moneyRatio(f.overageSpend, f.spendCap);
    if (r >= 1) {
      out.push({
        level: 'stop',
        feature: f.feature,
        threshold: 100,
        message: `${f.label} stopped: overage reached the spend cap.`,
        actions: [{ label: 'Raise the spend cap', href: BILLING_ROUTES.spend }, wait],
      });
    } else if (r >= 0.8) {
      out.push({
        level: 'warn',
        feature: f.feature,
        threshold: 80,
        message: `${f.label} overage is at 80% of the spend cap.`,
        actions: [{ label: 'Spend controls', href: BILLING_ROUTES.spend }],
      });
    }
  }

  const used = ratio(f.usage, f.included);
  if (!f.hasAccess) {
    if (out.some((n) => n.level === 'stop')) return out;
    const actions: BannerAction[] = [];
    if (f.prepaidOnly) {
      actions.push({ label: 'Buy credit', href: BILLING_ROUTES.payment });
    } else if (report.paymentState === 'none') {
      actions.push({ label: 'Add a card', href: BILLING_ROUTES.payment });
      actions.push({ label: 'Buy credit', href: BILLING_ROUTES.payment });
    } else if (f.mode === 'hard_limit') {
      actions.push({ label: 'Turn on overage', href: BILLING_ROUTES.spend });
    }
    actions.push(wait);
    out.push({
      level: 'stop',
      feature: f.feature,
      threshold: 100,
      message: f.prepaidOnly
        ? `${f.label} stopped: prepaid credit is used up.`
        : `${f.label} stopped at 100% of ${periodWord(f)} allowance.`,
      actions,
    });
    return out;
  }
  if (f.prepaidOnly) {
    // Prepaid credit warns when 20% is left (§13), not at 80% used.
    if (used >= 0.8) {
      out.push({
        level: 'warn',
        feature: f.feature,
        threshold: 20,
        message: `${f.label}: 20% of prepaid credit left.`,
        actions: [{ label: 'Buy credit', href: BILLING_ROUTES.payment }],
      });
    }
    return out;
  }
  if (used >= 1) {
    out.push({
      level: 'info',
      feature: f.feature,
      threshold: 100,
      message:
        f.mode === 'overage_allowed'
          ? `${f.label} is past ${periodWord(f)} allowance; overage is billed per unit.`
          : `${f.label} is past ${periodWord(f)} allowance.`,
      actions: [{ label: 'Usage', href: BILLING_ROUTES.usage }],
    });
  } else if (used >= 0.8) {
    out.push({
      level: 'warn',
      feature: f.feature,
      threshold: 80,
      message: `${f.label} is at 80% of ${periodWord(f)} allowance.`,
      actions: [{ label: 'Usage', href: BILLING_ROUTES.usage }],
    });
  }
  return out;
}

/** Every notice the report warrants, most severe first, then by feature. */
export function bannerNotices(report: UsageReport): BannerNotice[] {
  return report.features
    .flatMap((f) => noticesFor(f, report))
    .sort((a, b) => RANK[a.level] - RANK[b.level] || a.feature.localeCompare(b.feature));
}
