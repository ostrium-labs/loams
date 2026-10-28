// The operator view over the fixture client (ME1 Task 28): meters, plan versions, customer
// search and a manual grant (a reason required, then a confirmation), and meter health with
// its alerts (an ownerless partition, drift above 1%, a failed close run, a retrying exporter).

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { OperatorClient } from '../src/index.js';
import { createMockOperatorClient, OperatorSectionPage } from '../src/index.js';

afterEach(cleanup);

HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};

const NOW = Date.UTC(2026, 9, 27, 10, 30);

function show(
  section: Parameters<typeof OperatorSectionPage>[0]['section'],
  client: OperatorClient = createMockOperatorClient({ now: () => NOW }),
  customer?: string,
) {
  const navigate = vi.fn();
  render(
    <OperatorSectionPage
      section={section}
      customer={customer}
      client={client}
      navigate={navigate}
      sample
    />,
  );
  return navigate;
}

describe('the operator view', () => {
  it('lists meters with their aggregation and whether their semantics are fixed', async () => {
    show('meters');
    expect(await screen.findByText('house_cpu')).toBeTruthy();
    expect(screen.getByText('io.loams.dev.usage.house.query.v1')).toBeTruthy();
    expect(screen.getAllByText('In use').length).toBe(3);
    expect(screen.getByText('Archived')).toBeTruthy();
    expect(screen.getByText('UNIQUE_COUNT')).toBeTruthy();
    expect(screen.getByText(/Sample data/)).toBeTruthy();
  });

  it('shows plan versions, the current one first', async () => {
    show('plans');
    const free = (await screen.findByRole('heading', { name: 'Free (free)' })).closest(
      'section',
    ) as HTMLElement;
    expect(within(free).getByText('Version 3')).toBeTruthy();
    expect(within(free).getByText('Current')).toBeTruthy();
    expect(within(free).getByText('Version 2')).toBeTruthy();
    expect(screen.getByText(/overage €0.12 per unit/)).toBeTruthy();
  });

  it('searches customers and opens one', async () => {
    const navigate = show('customers');
    expect(await screen.findByRole('button', { name: 'Acme' })).toBeTruthy();
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search customers' }), {
      target: { value: 'globex-eu' },
    });
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Acme' })).toBeNull());
    fireEvent.click(screen.getByRole('button', { name: 'Globex' }));
    expect(navigate).toHaveBeenCalledWith('/operator/customers/cus-globex');
  });

  it('adds a manual grant only with a reason, after a confirmation, and lifts a stop', async () => {
    const client = createMockOperatorClient({ now: () => NOW });
    const create = vi.spyOn(client, 'createGrant');
    show('customers', client, 'cus-acme');
    const balances = (await screen.findByRole('heading', { name: 'Balances' })).closest(
      'section',
    ) as HTMLElement;
    expect(within(balances).getByText('Stopped')).toBeTruthy();

    fireEvent.change(screen.getByLabelText('Entitlement'), {
      target: { value: 'ent-free-llm_output_tokens' },
    });
    fireEvent.change(screen.getByLabelText(/Amount/), {
      target: { value: '50000' },
    });
    const add = screen.getByRole('button', {
      name: 'Add grant',
    }) as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText('Reason'), {
      target: { value: 'Support ticket 4411' },
    });
    expect(add.disabled).toBe(false);
    fireEvent.click(add);
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('Support ticket 4411');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add grant' }));
    await waitFor(() =>
      expect(create).toHaveBeenCalledWith(
        'cus-acme',
        expect.objectContaining({
          entitlement: 'ent-free-llm_output_tokens',
          amount: '50000',
          kind: 'promotional',
          expiresInDays: 30,
          reason: 'Support ticket 4411',
        }),
        expect.any(String),
      ),
    );
    await waitFor(() => expect(within(balances).queryByText('Stopped')).toBeNull());
  });

  it('meter health raises what needs an operator', async () => {
    show('health');
    expect(await screen.findByText('Partitions without an owner')).toBeTruthy();
    expect(screen.getByText(/globex\/house_cpu 1\.40%/)).toBeTruthy();
    expect(screen.getByText('House unavailable: connection refused')).toBeTruthy();
    expect(screen.getByText('retrying')).toBeTruthy();
    expect(screen.queryByText('Usage events dropped')).toBeNull();
  });
});
