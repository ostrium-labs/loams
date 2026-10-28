// The billing pages over the fixture client, with LiveChart stubbed (ECharts needs a canvas):
// each section renders, spend controls follow §13 (no overage without a card; a cap and a
// confirmation with one), payment links only to the provider, and the banner speaks up.

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import plugin, {
  BillingBanner,
  BillingSectionPage,
  createMockBillingClient,
} from '../src/index.js';
import type { BillingClient } from '../src/types.js';

vi.mock('@loams/charts', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@loams/charts')>()),
  LiveChart: (await import('./charts-stub.js')).LiveChart,
}));

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

const NOW = Date.UTC(2026, 9, 27, 10, 30);

function page(
  section: Parameters<typeof BillingSectionPage>[0]['section'],
  client: BillingClient,
  navigate = vi.fn(),
) {
  render(<BillingSectionPage section={section} client={client} navigate={navigate} sample />);
  return navigate;
}

describe('Settings › Billing', () => {
  it('Usage shows each feature with its chart, namespaces and the provisional window', async () => {
    page('usage', createMockBillingClient({ now: () => NOW }));
    expect(await screen.findByRole('heading', { name: 'House CPU' })).toBeTruthy();
    expect(screen.getByText(/Sample data/)).toBeTruthy();
    expect(screen.getByRole('tablist', { name: 'Billing sections' })).toBeTruthy();
    expect(screen.getByLabelText('House CPU: 85% used')).toBeTruthy();
    const chart = screen.getByLabelText(/House CPU per day/);
    expect(chart.getAttribute('data-chart-type')).toBe('Bar Chart');
    expect(Number(chart.getAttribute('data-rows'))).toBe(27);
    expect(screen.getByText('acme/analytics')).toBeTruthy();
    expect(screen.getAllByText('Stopped').length).toBe(1);
    expect(screen.getByText(/Later usage is provisional/)).toBeTruthy();
  });

  it('Entitlements lists the plan and grants in burn-down order', async () => {
    page('entitlements', createMockBillingClient({ now: () => NOW }));
    const card = (await screen.findByRole('heading', { name: 'LLM output tokens' })).closest(
      'section',
    ) as HTMLElement;
    const rows = within(card).getAllByRole('row').slice(1);
    expect(rows.map((r) => r.textContent?.split('Every period')[0]?.split('Once')[0])).toEqual([
      'Promotional',
      'Included',
    ]);
    expect(screen.getByText('Free')).toBeTruthy();
    expect(screen.getByText('max: 10')).toBeTruthy();
    expect(screen.getByText('Not included')).toBeTruthy();
  });

  it('Spend controls keep overage off without a card, and say why', async () => {
    const navigate = page(
      'spend',
      createMockBillingClient({ now: () => NOW, paymentState: 'none' }),
    );
    const blocked = await screen.findByTestId('overage-blocked');
    expect(blocked.textContent).toContain('Add a card');
    expect(
      screen.getByRole('switch', { name: /Allow overage/ }).getAttribute('aria-disabled'),
    ).not.toBe('false');
    fireEvent.click(within(blocked).getByRole('button', { name: 'Payment' }));
    expect(navigate).toHaveBeenCalledWith('/settings/billing/payment');
  });

  it('Spend controls turn overage on with a cap, after a confirmation', async () => {
    const client = createMockBillingClient({ now: () => NOW, paymentState: 'card' });
    const set = vi.spyOn(client, 'setSpendControls');
    page('spend', client);
    fireEvent.click(await screen.findByRole('switch', { name: /Allow overage/ }));
    const cap = screen.getByLabelText(/Spend cap per period/);
    fireEvent.change(cap, { target: { value: '60' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save spend controls' }));
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('€60.00');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save spend controls' }));
    await screen.findByText('Spend controls saved');
    expect(set).toHaveBeenCalledWith(
      expect.objectContaining({
        overageEnabled: true,
        spendCap: { micros: '60000000', currency: 'EUR' },
      }),
      expect.any(String),
    );
  });

  it('a person without billing_admin cannot change spend controls', async () => {
    page(
      'spend',
      createMockBillingClient({ now: () => NOW, paymentState: 'card', billingAdmin: false }),
    );
    expect(await screen.findByText('Read only')).toBeTruthy();
    expect(
      (screen.getByRole('button', { name: 'Save spend controls' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('Payment links only to the provider’s hosted page', async () => {
    page('payment', createMockBillingClient({ now: () => NOW }));
    const link = await screen.findByRole('link', { name: /Add a card on Stripe/ });
    expect(link.getAttribute('href')).toMatch(/^https:\/\/billing\.stripe\.com\//);
    expect(link.getAttribute('rel')).toContain('noopener');
    expect(document.querySelector('input[autocomplete^="cc-"]')).toBeNull();
  });

  it('Alerts adds a rule and shows a new endpoint’s secret once', async () => {
    page('alerts', createMockBillingClient({ now: () => NOW }));
    await screen.findByRole('heading', { name: 'Threshold rules' });
    fireEvent.change(screen.getByLabelText('Feature'), { target: { value: 'house_cpu' } });
    fireEvent.change(screen.getByLabelText(/Thresholds/), { target: { value: '50, 90' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add rule' }));
    expect(await screen.findByText('50%, 90%, and of the spend cap')).toBeTruthy();

    fireEvent.change(screen.getByLabelText('Endpoint URL'), {
      target: { value: 'https://example.org/hook' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add endpoint' }));
    expect(await screen.findByText('Signing secret')).toBeTruthy();
    expect(screen.getByText(/^whsec_sample/)).toBeTruthy();
    expect(screen.getByText('https://example.org/hook')).toBeTruthy();
    expect(screen.getByText('503')).toBeTruthy();
  });
});

describe('the banner', () => {
  it('says what stopped and how to resume', async () => {
    const navigate = vi.fn();
    render(
      <BillingBanner client={createMockBillingClient({ now: () => NOW })} navigate={navigate} />,
    );
    const banner = await screen.findByRole('alert');
    expect(banner.textContent).toContain('LLM output tokens stopped at 100%');
    fireEvent.click(within(banner).getByRole('button', { name: 'Add a card' }));
    expect(navigate).toHaveBeenCalledWith('/settings/billing/payment');
    expect(within(banner).queryByRole('button', { name: 'Dismiss' })).toBeNull();
  });

  it('a warning can be dismissed', async () => {
    const client = createMockBillingClient({ now: () => NOW });
    const base = await client.usage();
    const warnOnly: BillingClient = {
      ...client,
      usage: async () => ({ ...base, features: base.features.filter((f) => f.hasAccess) }),
    };
    render(<BillingBanner client={warnOnly} navigate={vi.fn()} />);
    const banner = await screen.findByRole('status');
    expect(banner.textContent).toContain('House CPU is at 80%');
    fireEvent.click(within(banner).getByRole('button', { name: 'Dismiss' }));
    await waitFor(() => expect(document.querySelector('[data-billing-banner]')).toBeNull());
  });
});

describe('the plugin', () => {
  it('registers Settings › Billing, its sections, the nav entry and the banner', () => {
    const pages = new Map<string, (p: { params: Record<string, string> }) => ReactNode>();
    const slots: { name: string; meta?: unknown }[] = [];
    const ctx = {
      effect: (fn: () => unknown) => void fn(),
      router: {
        page: (spec: { path: string }, r: (p: { params: Record<string, string> }) => ReactNode) =>
          void pages.set(spec.path, r),
        navigate: () => {},
      },
      slots: {
        register: (entry: { name: string; meta?: unknown }) => {
          slots.push(entry);
          return () => {};
        },
      },
    };
    (plugin as { apply(ctx: never, config: unknown): void }).apply(ctx as never, {});
    expect([...pages.keys()]).toEqual(['/settings/billing', '/settings/billing/:section']);
    expect(slots.map((s) => s.name)).toEqual(['shell.nav.section', 'shell.topbar.center']);
    expect(slots[0]?.meta).toMatchObject({ label: 'Billing', group: 'Settings' });
  });
});
