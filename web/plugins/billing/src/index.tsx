// @loams/plugin-billing: Loams Meter in the console (design 63 §18, D1132; ME1 Tasks 27, 28).
//
// Settings › Billing (design 59 D1080) has five sections, each its own route under
// /settings/billing: Usage, Entitlements, Spend controls, Payment (the provider's hosted page
// only) and Alerts. The shell's top bar shows the 80%/100% banner.
//
// The meter's API (loams.meter.v1, design 63 §17) is not served yet, so the plugin runs on the
// fixture client (src/mock.ts) and every page says so; the catalog loads it under Demo data
// (D2220). The pages read only the BillingClient interface (src/types.ts), so the Connect client
// replaces the fixture without touching them, and the plugin then moves to base.yml, gated on
// the instance's `meter` feature.
//
// The operator view (Task 28) lives under /operator: Meters, Plans, Customers (with manual
// grants, a reason required and audited) and Meter health. It reads an OperatorClient
// (src/operator/types.ts), on fixtures for now too.

import { type PageProps, type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import { AlertsPage } from './alerts.js';
import { BILLING_ROUTES } from './banner.js';
import { BillingBanner } from './banner-view.js';
import { EntitlementsPage } from './entitlements.js';
import { BillingLayout, type BillingSection, isSection } from './layout.js';
import { createMockBillingClient } from './mock.js';
import { createMockOperatorClient } from './operator/mock.js';
import { OPERATOR_ROUTES, type OperatorSection, OperatorSectionPage } from './operator/pages.js';
import { PaymentPage } from './payment.js';
import { SpendPage } from './spend.js';
import type { BillingClient } from './types.js';
import { UsagePage } from './usage.js';

export { AlertsPage, parseThresholds } from './alerts.js';
export {
  type BannerAction,
  type BannerLevel,
  type BannerNotice,
  BILLING_ROUTES,
  bannerNotices,
} from './banner.js';
export { BillingBanner, BillingBannerView } from './banner-view.js';
export { burnOrder, EntitlementsPage } from './entitlements.js';
export * from './format.js';
export { BillingLayout, SECTIONS } from './layout.js';
export { BillingError, createMockBillingClient, type MockBillingOptions } from './mock.js';
export { createMockOperatorClient, type MockOperatorOptions } from './operator/mock.js';
export {
  CustomerPage,
  CustomersPage,
  DRIFT_ALERT,
  HealthPage,
  MetersPage,
  OPERATOR_ROUTES,
  OperatorLayout,
  OperatorSectionPage,
  PlansPage,
} from './operator/pages.js';
export type * from './operator/types.js';
export { PaymentPage, safePortalUrl } from './payment.js';
export { overageBlocked, SpendPage } from './spend.js';
export type * from './types.js';
export { dailyRows, UsagePage } from './usage.js';

export interface BillingConfig {
  /** Fixture data. True until the meter serves loams.meter.v1 (D2220). */
  sample?: boolean;
}

/** One section's page, inside the Billing layout. */
export function BillingSectionPage({
  section,
  client,
  navigate,
  sample,
}: {
  section: BillingSection;
  client: BillingClient;
  navigate: (to: string) => void;
  sample?: boolean;
}) {
  return (
    <BillingLayout section={section} navigate={navigate} sample={sample}>
      {section === 'usage' && <UsagePage client={client} />}
      {section === 'entitlements' && <EntitlementsPage client={client} />}
      {section === 'spend' && <SpendPage client={client} navigate={navigate} />}
      {section === 'payment' && <PaymentPage client={client} />}
      {section === 'alerts' && <AlertsPage client={client} />}
    </BillingLayout>
  );
}

const plugin: PluginModule = {
  name: 'billing',
  inject: ['router', 'slots'],
  apply(ctx: Context, config?: BillingConfig) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const sample = config?.sample ?? true;
    // One client per plugin lifetime, so fixture changes survive moving between pages.
    const client = createMockBillingClient({ latencyMs: 120 });
    const navigate = (to: string) => router.navigate(to);

    ctx.effect(() =>
      router.page(
        {
          id: 'billing',
          path: BILLING_ROUTES.base,
          title: 'Billing',
          plugin: 'billing',
        },
        () => (
          <BillingSectionPage section="usage" client={client} navigate={navigate} sample={sample} />
        ),
      ),
    );
    ctx.effect(() =>
      router.page(
        {
          id: 'billing-section',
          path: `${BILLING_ROUTES.base}/:section`,
          title: 'Billing',
          plugin: 'billing',
        },
        ({ params }: PageProps) => (
          <BillingSectionPage
            section={isSection(params.section) ? params.section : 'usage'}
            client={client}
            navigate={navigate}
            sample={sample}
          />
        ),
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'billing',
          order: 120,
          meta: {
            id: 'billing',
            label: 'Billing',
            href: BILLING_ROUTES.base,
            icon: 'billing',
            group: 'Settings',
          },
        },
        () => null,
      ),
    );
    // The operator view (ME1 Task 28, D2223): Meters, Plans, Customers and Meter health.
    const operator = createMockOperatorClient({ latencyMs: 120 });
    const operatorPage = (id: string, path: string, section: OperatorSection) =>
      ctx.effect(() =>
        router.page(
          { id, path, title: 'Loams Meter', plugin: 'billing' },
          ({ params }: PageProps) => (
            <OperatorSectionPage
              section={section}
              customer={params.customer}
              client={operator}
              navigate={navigate}
              sample={sample}
            />
          ),
        ),
      );
    operatorPage('meter-meters', OPERATOR_ROUTES.meters, 'meters');
    operatorPage('meter-plans', OPERATOR_ROUTES.plans, 'plans');
    operatorPage('meter-customers', OPERATOR_ROUTES.customers, 'customers');
    operatorPage('meter-customer', `${OPERATOR_ROUTES.customers}/:customer`, 'customers');
    operatorPage('meter-health', OPERATOR_ROUTES.health, 'health');
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'billing',
          order: 30,
          meta: {
            id: 'meter',
            label: 'Loams Meter',
            href: OPERATOR_ROUTES.meters,
            icon: 'meter',
            group: 'Instance',
          },
        },
        () => null,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.topbar.center',
          plugin: 'billing',
          order: 50,
          meta: { mode: 'console' },
        },
        () => <BillingBanner client={client} navigate={navigate} />,
      ),
    );
  },
};

export default plugin;
