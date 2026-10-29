// The palette's tables (CF1 Task 11, D1078): the namespace's collections from
// `ListCollections`, opening the collection, Search or the SQL editor. Kept apart from
// data.test.tsx, which the Search work (#769) rewrites (ruling T11-1).

import type { PaletteCommand } from '@loams/slots';
import { codeEditorText } from '@loams/ui';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import type { DataClient } from '../src/client.js';
import { TablesSource, tableStatement } from '../src/index.js';
import { SqlPage } from '../src/pages/sql.js';

afterEach(cleanup);

Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const tables = () =>
  ({
    listCollections: vi.fn(async () => [{ name: 'orders' }, { name: 'users' }]),
  }) as unknown as DataClient;

describe('the palette tables', () => {
  it("the palette lists the namespace's tables, once something is typed (D1078, T11-3)", async () => {
    const published: PaletteCommand[][] = [];
    const navigate = vi.fn();
    const client = tables();
    const source = (query: string) => (
      <TablesSource
        query={query}
        publish={(c) => published.push([...c])}
        client={client}
        namespace="shop"
        navigate={navigate}
      />
    );
    const { rerender } = render(source(''));
    await waitFor(() => expect(client.listCollections).toHaveBeenCalledWith('shop'));
    expect(published.at(-1)).toEqual([]);
    rerender(source('ord'));
    await waitFor(() =>
      expect(
        published
          .at(-1)
          ?.filter((c) => !c.secondary)
          .map((c) => c.title),
      ).toEqual(['orders', 'users']),
    );
    expect(client.listCollections).toHaveBeenCalledTimes(1);
    published.at(-1)?.[0]?.run();
    expect(navigate).toHaveBeenCalledWith('/data/shop/orders');
  });

  it('each table also offers Search and the SQL editor, after the table (T11-2)', async () => {
    const published: PaletteCommand[][] = [];
    const navigate = vi.fn();
    render(
      <TablesSource
        query="ord"
        publish={(c) => published.push([...c])}
        client={tables()}
        namespace="shop"
        navigate={navigate}
      />,
    );
    await waitFor(() => expect(published.at(-1)?.length).toBe(6));
    const rows = published.at(-1) ?? [];
    expect(rows.slice(0, 3).map((c) => [c.title, c.secondary ?? false])).toEqual([
      ['orders', false],
      ['Search orders', true],
      ['Query orders in SQL', true],
    ]);
    rows[1]?.run();
    rows[2]?.run();
    expect(navigate.mock.calls).toEqual([
      ['/data/shop/orders/search'],
      ['/data/shop/sql?table=orders'],
    ]);
  });

  it('publishes nothing without a namespace (no data plane)', async () => {
    const publish = vi.fn();
    const client = tables();
    render(
      <TablesSource
        query=""
        publish={publish}
        client={client}
        namespace={undefined}
        navigate={() => {}}
      />,
    );
    await waitFor(() => expect(publish).toHaveBeenCalledWith([]));
    expect(client.listCollections).not.toHaveBeenCalled();
  });

  it('the SQL editor opens on a SELECT of ?table=, and follows a new one', async () => {
    expect(tableStatement('we"ird')).toBe('SELECT * FROM "we""ird" LIMIT 100');
    const client = {
      sql: vi.fn(async () => ({ columns: [], rows: [], truncated: false })),
    } as unknown as DataClient;
    const { rerender } = render(
      <SqlPage client={client} ns="shop" navigate={() => {}} table="orders" />,
    );
    expect(codeEditorText(screen.getByLabelText('SQL'))).toBe(tableStatement('orders'));
    rerender(<SqlPage client={client} ns="shop" navigate={() => {}} table="users" />);
    await act(async () => {});
    expect(codeEditorText(screen.getByLabelText('SQL'))).toBe(tableStatement('users'));
    // The code editor's first mount is slow on a loaded runner.
  }, 20_000);
});
