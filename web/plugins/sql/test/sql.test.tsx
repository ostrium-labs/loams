// The Loams SQL page against the fixture-backed client (src/mock.ts).

import { validateManifest } from '@loams/console-host';
import { setCodeEditorText } from '@loams/ui';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import {
  canResume,
  canSuspend,
  connectionUri,
  isTransitional,
  mysqlCommand,
  PAGE_MAX_ROWS,
  type SqlClient,
} from '../src/client.js';
import { createMockSqlClient, defaultFixtures } from '../src/mock.js';
import { SqlPage } from '../src/SqlPage.js';

afterEach(cleanup);

// jsdom has no layout: the code editor measures text ranges for its selection and tooltips.
Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

function mount(client: SqlClient = createMockSqlClient({ transitionMs: 0 })) {
  const copy = vi.fn();
  render(<SqlPage client={client} copy={copy} mocked />);
  return { client, copy };
}

const dbButton = (name: string) =>
  within(screen.getByRole('navigation', { name: 'Databases' })).getByRole('button', {
    name: new RegExp(`^${name}`),
  });

describe('manifest', () => {
  it('is a valid console plugin manifest', () => {
    expect(() => validateManifest(pkg)).not.toThrow();
  });
});

describe('client helpers', () => {
  it('maps stages to actions', () => {
    expect(canSuspend('running')).toBe(true);
    expect(canSuspend('suspended')).toBe(false);
    expect(canResume('suspended')).toBe(true);
    expect(canResume('failed')).toBe(true);
    expect(canResume('running')).toBe(false);
    expect(isTransitional('resuming')).toBe(true);
    expect(isTransitional('running')).toBe(false);
  });

  it('keeps the password out of the URI and command unless given', () => {
    const c = {
      host: 'h.example.invalid',
      port: 4000,
      user: 'u.main',
      database: 'shop',
      password: 'p',
      tlsRequired: true,
    };
    expect(connectionUri(c)).toBe('mysql://u.main@h.example.invalid:4000/shop?ssl-mode=REQUIRED');
    expect(connectionUri(c, 'p w')).toContain('u.main:p%20w@');
    expect(mysqlCommand(c)).not.toContain(' p ');
    expect(mysqlCommand(c)).toContain('-p shop');
  });
});

describe('mock client', () => {
  it('returns a password once, then only on rotation', async () => {
    const c = createMockSqlClient();
    const first = await c.getConnectionInfo(
      'default',
      'db_shop000000000001',
      'br_shopmain00000000',
      false,
    );
    expect(first.password).not.toBe('');
    const again = await c.getConnectionInfo(
      'default',
      'db_shop000000000001',
      'br_shopmain00000000',
      false,
    );
    expect(again.password).toBe('');
    const rotated = await c.getConnectionInfo(
      'default',
      'db_shop000000000001',
      'br_shopmain00000000',
      true,
    );
    expect(rotated.password).not.toBe('');
    expect(rotated.password).not.toBe(first.password);
  });

  it('walks suspend through suspending to suspended', async () => {
    let t = 0;
    const c = createMockSqlClient({ transitionMs: 100, now: () => t });
    const d = await c.suspendDatabase('default', 'db_shop000000000001', 'k');
    expect(d.stage).toBe('suspending');
    t = 100;
    expect((await c.getDatabase('default', 'db_shop000000000001')).stage).toBe('suspended');
  });

  it('refuses queries on a suspended database', async () => {
    const c = createMockSqlClient();
    await expect(
      c.query('default', {
        databaseId: 'db_analytics0000001',
        branchId: 'br_anlmain000000000',
        statement: 'SELECT 1',
        maxRows: PAGE_MAX_ROWS,
        timeoutMs: 1000,
      }),
    ).rejects.toMatchObject({ reason: 'database_suspended' });
  });
});

describe('SqlPage', () => {
  it('lists databases with their state and selects the first', async () => {
    mount();
    expect(await screen.findByRole('navigation', { name: 'Databases' })).toBeTruthy();
    expect(dbButton('shop').getAttribute('aria-current')).toBe('true');
    expect(within(dbButton('analytics')).getByText('Suspended')).toBeTruthy();
    expect(screen.getByRole('heading', { level: 2, name: 'shop' })).toBeTruthy();
  });

  it('lists branches of the selected database', async () => {
    mount();
    const table = await screen.findByRole('table', { name: 'Branches' });
    // `main` is both a branch name and `dev`'s parent, so match by row action.
    expect(within(table).getByRole('button', { name: 'Use branch main' })).toBeTruthy();
    expect(within(table).getByRole('button', { name: 'Use branch dev' })).toBeTruthy();
    expect(within(table).getByText('462118237114122241')).toBeTruthy();
  });

  it('shows the password once with a copy button, then offers rotation', async () => {
    const { copy } = mount();
    const pw = await screen.findByTestId('sql-password');
    const secret = pw.textContent ?? '';
    expect(secret).not.toBe('');
    fireEvent.click(screen.getByRole('button', { name: 'Copy password' }));
    expect(copy).toHaveBeenCalledWith(secret);
    fireEvent.click(screen.getByRole('button', { name: 'Hide' }));
    expect(screen.queryByTestId('sql-password')).toBeNull();
    expect(screen.getByRole('button', { name: 'Rotate password' })).toBeTruthy();
  });

  it('keeps the minted password when StrictMode drops the first load as stale', async () => {
    render(
      <StrictMode>
        <SqlPage client={createMockSqlClient({ transitionMs: 0 })} copy={vi.fn()} mocked />
      </StrictMode>,
    );
    const pw = await screen.findByTestId('sql-password');
    expect(pw.textContent).toContain('mock-pw');
  });

  it('copies the connection string without the password', async () => {
    const { copy } = mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Copy connection string' }));
    const uri = copy.mock.calls[0]?.[0] as string;
    expect(uri.startsWith('mysql://')).toBe(true);
    expect(uri).not.toContain('mock-pw');
  });

  it('suspends a running database and resumes it', async () => {
    const client = createMockSqlClient({ transitionMs: 0 });
    const suspend = vi.spyOn(client, 'suspendDatabase');
    mount(client);
    fireEvent.click(await screen.findByRole('button', { name: 'Suspend' }));
    await waitFor(() => expect(suspend).toHaveBeenCalledTimes(1));
    expect(suspend.mock.calls[0]?.[2]).toBeTruthy();
    const resume = await screen.findByRole('button', { name: 'Resume' }, { timeout: 3000 });
    expect(screen.getByRole('button', { name: 'Run' })).toHaveProperty('disabled', true);
    fireEvent.click(resume);
    await screen.findByRole('button', { name: 'Suspend' }, { timeout: 3000 });
  });

  it('shows Waking while a database resumes', async () => {
    mount(createMockSqlClient({ transitionMs: 60_000 }));
    fireEvent.click(await screen.findByRole('button', { name: /^analytics/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Resume' }));
    const state = await screen.findByRole('status', { name: 'State of analytics' });
    await waitFor(() => expect(state.textContent).toBe('Waking'));
  });

  it('runs SQL with Ctrl+Enter and renders a results grid with NULLs', async () => {
    const client = createMockSqlClient({ transitionMs: 0 });
    const query = vi.spyOn(client, 'query');
    mount(client);
    const box = await screen.findByLabelText('Statement');
    act(() => setCodeEditorText(box, 'SELECT * FROM products'));
    fireEvent.keyDown(box, { key: 'Enter', ctrlKey: true });
    const grid = await screen.findByRole('table', { name: 'Result rows' });
    const ran = query.mock.calls.find((c) => c[1].statement === 'SELECT * FROM products');
    expect(ran?.[1]).toMatchObject({
      databaseId: 'db_shop000000000001',
      branchId: 'br_shopmain00000000',
      statement: 'SELECT * FROM products',
      maxRows: PAGE_MAX_ROWS,
    });
    expect(
      within(grid)
        .getAllByRole('columnheader')
        .map((h) => h.textContent),
    ).toEqual(['#', 'idBIGINT', 'nameVARCHAR', 'priceDECIMAL', 'discontinued_atDATETIME']);
    expect(within(grid).getAllByText('NULL').length).toBe(2);
  });

  it('shows a failed statement', async () => {
    mount();
    const box = await screen.findByLabelText('Statement');
    act(() => setCodeEditorText(box, 'SELEC oops'));
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(await screen.findByText(/syntax_error/)).toBeTruthy();
  });

  it('runs against the chosen branch', async () => {
    const client = createMockSqlClient({ transitionMs: 0 });
    const info = vi.spyOn(client, 'getConnectionInfo');
    mount(client);
    fireEvent.click(await screen.findByRole('button', { name: 'Use branch dev' }));
    await waitFor(() =>
      expect(info).toHaveBeenLastCalledWith(
        'default',
        'db_shop000000000001',
        'br_shopdev000000000',
        false,
      ),
    );
  });

  it('shows an empty state with no databases', async () => {
    mount(createMockSqlClient({ fixtures: { databases: [], branches: [] } }));
    expect(await screen.findByText('No SQL databases yet')).toBeTruthy();
  });

  it('shows a retry when listing fails', async () => {
    const client = createMockSqlClient();
    vi.spyOn(client, 'listDatabases').mockRejectedValueOnce(new Error('offline'));
    mount(client);
    expect(await screen.findByText('offline')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('navigation', { name: 'Databases' })).toBeTruthy();
  });

  it('fixtures carry no real hosts', async () => {
    const fx = defaultFixtures();
    const c = createMockSqlClient({ fixtures: fx });
    for (const b of fx.branches) {
      const info = await c.getConnectionInfo('default', b.databaseId, b.id, false);
      expect(info.host.endsWith('.invalid')).toBe(true);
    }
  });
});
