import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { Schemas } from '../src/index.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const wesql = {
  schemas: async () => ({ ok: true as const, value: [{ name: 'app' }, { name: 'analytics' }] }),
  tables: async (s: string) => ({
    ok: true as const,
    value: s === 'app' ? [{ name: 'orders', engine: 'InnoDB', rows: 18234 }] : [],
  }),
};

describe('wesql schemas', () => {
  it('lists schemas, then tables with engine and rows', async () => {
    render(<Schemas wesql={wesql as never} />);
    expect(await screen.findByText('orders')).toBeTruthy();
    expect(screen.getByText('InnoDB')).toBeTruthy();
    expect(screen.getByText('18,234')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'analytics' }));
    expect(await screen.findByText('No tables')).toBeTruthy();
  });
});
