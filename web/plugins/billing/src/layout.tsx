// Settings › Billing (design 59 D1080, design 63 §18): one page with a tab per section, each
// tab its own route so links and the palette reach it.

import { SampleDataNotice, Tabs } from '@loams/ui';
import type { ReactNode } from 'react';
import { BILLING_ROUTES } from './banner.js';

export type BillingSection = 'usage' | 'entitlements' | 'spend' | 'payment' | 'alerts';

export const SECTIONS: { value: BillingSection; label: string; href: string }[] = [
  { value: 'usage', label: 'Usage', href: BILLING_ROUTES.usage },
  { value: 'entitlements', label: 'Entitlements', href: BILLING_ROUTES.entitlements },
  { value: 'spend', label: 'Spend controls', href: BILLING_ROUTES.spend },
  { value: 'payment', label: 'Payment', href: BILLING_ROUTES.payment },
  { value: 'alerts', label: 'Alerts', href: BILLING_ROUTES.alerts },
];

export function isSection(v: string | undefined): v is BillingSection {
  return SECTIONS.some((s) => s.value === v);
}

export function BillingLayout({
  section,
  navigate,
  sample,
  children,
}: {
  section: BillingSection;
  navigate: (to: string) => void;
  sample?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="lc-page">
      <header className="lc-page-head">
        <div>
          <h1>Billing</h1>
          <p>Usage, entitlements and spend for this organisation, from Loams Meter.</p>
        </div>
      </header>
      {sample && (
        <div className="mb-3">
          <SampleDataNotice>
            The usage, plan and payment shown are fixtures, and changes last until you reload.
          </SampleDataNotice>
        </div>
      )}
      <Tabs
        label="Billing sections"
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
