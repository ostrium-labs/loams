import { describe, expect, it } from 'vite-plus/test';
import { BILLING_ROUTES, bannerNotices } from '../src/banner.js';
import { formatDay, money } from '../src/format.js';
import type { FeatureUsage, UsageReport } from '../src/types.js';

const period = { start: '2026-10-01T00:00:00Z', end: '2026-11-01T00:00:00Z' };

function f(over: Partial<FeatureUsage>): FeatureUsage {
  return {
    feature: 'house_cpu',
    label: 'House CPU',
    unit: 'cu_ms',
    mode: 'hard_limit',
    period,
    usage: '0',
    included: '100',
    balance: '100',
    overage: '0',
    hasAccess: true,
    daily: [],
    byNamespace: [],
    ...over,
  };
}

const report = (features: FeatureUsage[], paymentState: UsageReport['paymentState'] = 'none') => ({
  customer: 'acme',
  paymentState,
  asOf: '2026-10-27T10:00:00Z',
  features,
});

describe('the billing banner', () => {
  it('is quiet below 80%', () => {
    expect(bannerNotices(report([f({ usage: '79.99' })]))).toEqual([]);
  });

  it('warns at 80% of the month', () => {
    const [n] = bannerNotices(report([f({ usage: '80' })]));
    expect(n?.level).toBe('warn');
    expect(n?.message).toBe("House CPU is at 80% of this month's allowance.");
  });

  it('at a hard stop without a card, says how to resume', () => {
    const [n] = bannerNotices(report([f({ usage: '100', hasAccess: false })]));
    expect(n?.level).toBe('stop');
    expect(n?.message).toContain('stopped at 100%');
    expect(n?.actions.map((a) => a.label)).toEqual([
      'Add a card',
      'Buy credit',
      `Wait for the reset on ${formatDay(period.end)}`,
    ]);
    expect(n?.actions[0]?.href).toBe(BILLING_ROUTES.payment);
  });

  it('with a card on a hard limit, offers overage', () => {
    const [n] = bannerNotices(report([f({ usage: '100', hasAccess: false })], 'card'));
    expect(n?.actions[0]).toEqual({ label: 'Turn on overage', href: BILLING_ROUTES.spend });
  });

  it('stops at the spend cap and warns at 80% of it', () => {
    const capped = f({
      mode: 'overage_allowed',
      usage: '150',
      overage: '50',
      overageSpend: money(40),
      spendCap: money(40),
      hasAccess: false,
    });
    const [stop] = bannerNotices(report([capped], 'card'));
    expect(stop?.message).toContain('spend cap');
    expect(stop?.actions[0]?.label).toBe('Raise the spend cap');

    const near = f({
      mode: 'overage_allowed',
      usage: '120',
      overage: '20',
      overageSpend: money(32),
      spendCap: money(40),
    });
    const notices = bannerNotices(report([near], 'card'));
    expect(notices.map((n) => [n.level, n.threshold])).toEqual([
      ['warn', 80],
      ['info', 100],
    ]);
  });

  it('for prepaid credit, warns when 20% is left', () => {
    const [n] = bannerNotices(report([f({ prepaidOnly: true, usage: '85' })], 'card'));
    expect(n).toMatchObject({ level: 'warn', threshold: 20 });
    const [stop] = bannerNotices(
      report([f({ prepaidOnly: true, usage: '100', hasAccess: false })]),
    );
    expect(stop?.message).toContain('prepaid credit is used up');
    expect(stop?.actions[0]?.label).toBe('Buy credit');
  });

  it('puts a stop before a warning', () => {
    const notices = bannerNotices(
      report([
        f({ feature: 'a', label: 'A', usage: '90' }),
        f({ feature: 'b', label: 'B', usage: '100', hasAccess: false }),
      ]),
    );
    expect(notices.map((n) => n.feature)).toEqual(['b', 'a']);
  });
});
