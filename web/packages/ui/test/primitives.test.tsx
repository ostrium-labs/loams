import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { Button, DataTable, Dialog, Menu, MenuItem, Segmented, Tabs, VirtualList } from '../src';

afterEach(cleanup);

describe('Dialog', () => {
  it('opens as a labelled modal and reports a dismissal', async () => {
    let closed = 0;
    const { rerender } = render(
      <Dialog open onClose={() => closed++} title="Delete key">
        <p>Sure?</p>
      </Dialog>,
    );
    const dialog = await screen.findByRole('dialog');
    expect(dialog.getAttribute('aria-labelledby')).toBeTruthy();
    expect(screen.getByText('Delete key').tagName).toBe('H2');
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(closed).toBe(1);
    rerender(
      <Dialog open={false} onClose={() => closed++} title="Delete key">
        <p>Sure?</p>
      </Dialog>,
    );
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('cannot be dismissed while not dismissible', async () => {
    let closed = 0;
    render(
      <Dialog open onClose={() => closed++} title="Working" dismissible={false}>
        <p>Wait</p>
      </Dialog>,
    );
    await screen.findByRole('dialog');
    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
    expect(closed).toBe(0);
  });
});

describe('Menu', () => {
  it('opens from its trigger and runs an item', async () => {
    let picked = '';
    render(
      <Menu trigger={<Button>Actions</Button>}>
        <MenuItem onSelect={() => (picked = 'rename')}>Rename</MenuItem>
      </Menu>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Actions' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Rename' }));
    expect(picked).toBe('rename');
  });
});

describe('Tabs', () => {
  it('shows the active panel', () => {
    function T() {
      const [v, setV] = useState<'a' | 'b'>('a');
      return (
        <Tabs
          label="Views"
          value={v}
          onValueChange={setV}
          tabs={[
            { value: 'a', label: 'Alpha', panel: <p>first</p> },
            { value: 'b', label: 'Beta', panel: <p>second</p> },
          ]}
        />
      );
    }
    render(<T />);
    expect(screen.getByText('first')).toBeTruthy();
    fireEvent.click(screen.getByRole('tab', { name: 'Beta' }));
    expect(screen.getByRole('tab', { name: 'Beta' }).getAttribute('aria-selected')).toBe('true');
  });
});

describe('Segmented', () => {
  it('presses one option and never none', () => {
    const seen: string[] = [];
    function S() {
      const [v, setV] = useState<'console' | 'studio'>('console');
      return (
        <Segmented
          label="Mode"
          value={v}
          onValueChange={(next) => {
            seen.push(next);
            setV(next);
          }}
          options={[
            { value: 'console', label: 'Console' },
            { value: 'studio', label: 'Studio' },
          ]}
        />
      );
    }
    render(<S />);
    expect(screen.getByRole('group', { name: 'Mode' })).toBeTruthy();
    const studio = screen.getByRole('button', { name: 'Studio' });
    expect(studio.getAttribute('aria-pressed')).toBe('false');
    fireEvent.click(studio);
    expect(studio.getAttribute('aria-pressed')).toBe('true');
    // Pressing the pressed option again keeps it pressed.
    fireEvent.click(studio);
    expect(studio.getAttribute('aria-pressed')).toBe('true');
    expect(seen).toEqual(['studio']);
  });
});

describe('DataTable', () => {
  const rows = [
    { id: 'b', n: 2 },
    { id: 'a', n: 10 },
    { id: 'c', n: 1 },
  ];
  it('sorts by a column header', () => {
    render(
      <DataTable
        caption="Numbers"
        rows={rows}
        rowKey={(r) => r.id}
        columns={[
          { key: 'id', header: 'Id', cell: (r) => r.id, sortValue: (r) => r.id },
          { key: 'n', header: 'N', cell: (r) => r.n, numeric: true, sortValue: (r) => r.n },
        ]}
      />,
    );
    const firstCells = () =>
      screen
        .getAllByRole('row')
        .slice(1)
        .map((r) => r.querySelector('td')?.textContent);
    expect(firstCells()).toEqual(['b', 'a', 'c']);
    act(() => fireEvent.click(screen.getByRole('button', { name: /N/ })));
    expect(firstCells()).toEqual(['c', 'b', 'a']);
    expect(screen.getAllByRole('columnheader')[1]?.getAttribute('aria-sort')).toBe('ascending');
  });
});

describe('VirtualList', () => {
  it('renders a labelled scrolling region', () => {
    render(
      <VirtualList
        label="Events"
        items={['x', 'y']}
        itemKey={(s) => s}
        render={(s) => <span>{s}</span>}
      />,
    );
    expect(screen.getByRole('region', { name: 'Events' })).toBeTruthy();
  });
});
