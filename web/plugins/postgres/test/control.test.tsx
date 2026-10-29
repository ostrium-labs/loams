// The control plane view (PG2 Tasks 5-7) against the fixture-backed mock client.

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import {
  BranchTree,
  branchTree,
  ControlPlane,
  createMockPgControl,
  deleteBlocker,
  fixtureState,
  type PgControlClient,
  PgControlError,
} from '../src/index.js';

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

const TEST_PASSWORD = 'test-only-not-a-secret';

function mount(client: PgControlClient = createMockPgControl({ password: () => TEST_PASSWORD })) {
  const copy = vi.fn(async () => {});
  render(<ControlPlane client={client} pollMs={10} copy={copy} />);
  return { client, copy };
}

/** Narrows `value`, failing the test with a clear message when it is missing. */
function present<T>(value: T | undefined): T {
  if (value === undefined) throw new Error('expected a value');
  return value;
}

const branchRow = (id: string) => document.querySelector(`tr[data-branch="${id}"]`) as HTMLElement;

describe('branch tree', () => {
  it('orders branches depth-first by parent, default first', () => {
    const rows = branchTree(fixtureState().branches);
    expect(rows.map((r) => [r.branch.name, r.depth])).toEqual([
      ['main', 0],
      ['dev', 1],
      ['feature-checkout', 2],
      ['preview-42', 1],
    ]);
    expect(rows[0]?.children).toEqual(['br-dev', 'br-preview']);
  });

  it('blocks deleting the default branch and a branch with children', () => {
    const rows = branchTree(fixtureState().branches);
    const by = (name: string) => present(rows.find((r) => r.branch.name === name));
    expect(deleteBlocker(by('main'))).toMatch(/default/);
    expect(deleteBlocker(by('dev'))).toMatch(/child/);
    expect(deleteBlocker(by('feature-checkout'))).toBeUndefined();
  });

  it('shows the branch point and the protected flag', () => {
    render(
      <BranchTree
        branches={fixtureState().branches}
        onSelect={() => {}}
        onCreate={async () => {}}
        onProtect={async () => {}}
        onDelete={async () => {}}
      />,
    );
    const dev = branchRow('br-dev');
    expect(within(dev).getByText('0/2F00A10')).toBeTruthy();
    const main = branchRow('br-main');
    expect((within(main).getByLabelText('Protect main') as HTMLInputElement).checked).toBe(true);
    expect(
      (within(main).getByRole('button', { name: 'Delete branch main' }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });
});

describe('control plane', () => {
  it('labels a beta Postgres major and creates with it only after the opt-in', async () => {
    const { client } = mount();
    fireEvent.click(await screen.findByRole('button', { name: /New project/ }));
    fireEvent.change(await screen.findByLabelText('Project name'), { target: { value: 'next' } });
    const select = (await screen.findByLabelText('Postgres version')) as HTMLSelectElement;
    await waitFor(() =>
      expect(within(select).getByRole('option', { name: '18 (beta)' })).toBeTruthy(),
    );
    // The maintenance major is not offered to a new project; the default is preselected.
    expect(select.value).toBe('17');
    fireEvent.change(select, { target: { value: '18' } });
    const create = screen.getByRole('button', { name: 'Create project' }) as HTMLButtonElement;
    expect(create.disabled).toBe(true);
    fireEvent.click(screen.getByLabelText('Use Postgres 18, which is in beta'));
    expect(create.disabled).toBe(false);
    fireEvent.click(create);
    await waitFor(async () =>
      expect(
        (await client.listProjects('default')).some((p) => p.name === 'next' && p.pgVersion === 18),
      ).toBe(true),
    );
  });

  it('refuses a beta major without the opt-in in the mock as the server does', async () => {
    const client = createMockPgControl({ password: () => TEST_PASSWORD });
    await expect(
      client.createProject('default', { name: 'nope', pgVersion: 18 }),
    ).rejects.toMatchObject({
      reason: 'beta_major_requires_opt_in',
    });
  });

  it('lists the project, its branches and says it runs on sample data', async () => {
    mount();
    expect(await screen.findByText('Sample data')).toBeTruthy();
    await waitFor(() => expect(branchRow('br-feature')).toBeTruthy());
    expect(branchRow('br-feature').getAttribute('data-depth')).toBe('2');
    // The default branch is selected: its roles and databases load.
    expect(await screen.findByText('Roles and databases on main')).toBeTruthy();
  });

  it('creates a branch at an LSN and follows its operation to ready', async () => {
    const client = createMockPgControl();
    const create = vi.spyOn(client, 'createBranch');
    mount(client);
    await waitFor(() => expect(branchRow('br-dev')).toBeTruthy());
    fireEvent.click(
      within(branchRow('br-dev')).getByRole('button', { name: 'Create branch from dev' }),
    );
    fireEvent.change(await screen.findByLabelText('Branch name'), { target: { value: 'try-it' } });
    fireEvent.click(screen.getByLabelText('An LSN'));
    fireEvent.change(screen.getByLabelText('LSN'), { target: { value: '0/30ff000' } });
    fireEvent.click(screen.getByLabelText('Protected (only an admin can delete it)'));
    fireEvent.click(screen.getByRole('button', { name: 'Create branch' }));
    await waitFor(() =>
      expect(create).toHaveBeenCalledWith('default', 'prj-shop', {
        name: 'try-it',
        parentId: 'br-dev',
        protected: true,
        point: { lsn: '0/30FF000' },
      }),
    );
    // The operation runs to completion under polling (the fixtures hold one finished op).
    await waitFor(
      () =>
        expect(document.querySelectorAll('tr[data-operation][data-state="succeeded"]').length).toBe(
          2,
        ),
      { timeout: 2000 },
    );
    const ops = await client.listOperations('default');
    expect(ops[0]?.state).toBe('succeeded');
  });

  it('refuses an invalid LSN before calling the server', async () => {
    mount();
    await waitFor(() => expect(branchRow('br-dev')).toBeTruthy());
    fireEvent.click(
      within(branchRow('br-dev')).getByRole('button', { name: 'Create branch from dev' }),
    );
    fireEvent.change(await screen.findByLabelText('Branch name'), { target: { value: 'x' } });
    fireEvent.click(screen.getByLabelText('An LSN'));
    fireEvent.change(screen.getByLabelText('LSN'), { target: { value: 'nope' } });
    expect(screen.getByText('An LSN looks like 0/16B3748.')).toBeTruthy();
    expect(
      (screen.getByRole('button', { name: 'Create branch' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('shows a created role password once and drops it on close', async () => {
    const { copy } = mount();
    await screen.findByText('Roles and databases on main');
    fireEvent.change(screen.getByLabelText('New role'), { target: { value: 'reporter' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create role' }));
    const field = (await screen.findByLabelText('Password')) as HTMLInputElement;
    expect(field.value).toBe(TEST_PASSWORD);
    expect(field.type).toBe('password');
    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    await waitFor(() => expect(copy).toHaveBeenCalledWith(TEST_PASSWORD));
    fireEvent.click(screen.getByRole('button', { name: 'I have saved it' }));
    await waitFor(() => expect(screen.queryByDisplayValue(TEST_PASSWORD)).toBeNull());
    expect(document.querySelector('tr[data-role="reporter"]')).toBeTruthy();
  });

  it('says when the clipboard refuses the password instead of claiming it copied', async () => {
    const client = createMockPgControl({ password: () => TEST_PASSWORD });
    const copy = vi.fn(async () => {
      throw new Error('clipboard denied');
    });
    render(<ControlPlane client={client} pollMs={10} copy={copy} />);
    await screen.findByText('Roles and databases on main');
    fireEvent.change(screen.getByLabelText('New role'), { target: { value: 'reporter' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create role' }));
    await screen.findByLabelText('Password');
    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    expect(await screen.findByText(/Could not copy/)).toBeTruthy();
    expect(screen.queryByText('Copied to the clipboard.')).toBeNull();
  });

  it('resets a password and says the old one stops working', async () => {
    mount();
    await screen.findByText('Roles and databases on main');
    fireEvent.click(screen.getByRole('button', { name: 'Reset password for app' }));
    expect(await screen.findByText(/The old password no longer works/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Reset password for loams_admin' })).toBeNull();
  });

  it('creates and deletes a database owned by a role on the branch', async () => {
    const client = createMockPgControl();
    mount(client);
    await screen.findByText('Roles and databases on main');
    fireEvent.change(screen.getByLabelText('New database'), { target: { value: 'analytics' } });
    fireEvent.change(screen.getByLabelText('Owner'), { target: { value: 'app' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create database' }));
    const analytics = () => document.querySelector('tr[data-database="analytics"]');
    await waitFor(() => expect(analytics()).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'Delete database analytics' }));
    await waitFor(() => expect(analytics()).toBeNull());
  });

  it('surfaces a refusal reason', async () => {
    const client = createMockPgControl();
    vi.spyOn(client, 'deleteRole').mockRejectedValue(
      new PgControlError('role_owns_database', 'app owns a database on this branch'),
    );
    mount(client);
    await screen.findByText('Roles and databases on main');
    fireEvent.click(screen.getByRole('button', { name: 'Delete role app' }));
    expect(await screen.findByText(/role_owns_database/)).toBeTruthy();
  });

  it('deletes a leaf branch through an operation', async () => {
    const client = createMockPgControl();
    mount(client);
    await waitFor(() => expect(branchRow('br-preview')).toBeTruthy());
    fireEvent.click(
      within(branchRow('br-preview')).getByRole('button', { name: 'Delete branch preview-42' }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Delete branch' }));
    await waitFor(() => expect(branchRow('br-preview')).toBeNull(), { timeout: 2000 });
  });
});

describe('mock control plane rules', () => {
  it('refuses deleting a branch with children', async () => {
    const c = createMockPgControl();
    const [dev] = (await c.listBranches('default', 'prj-shop')).filter((b) => b.id === 'br-dev');
    await expect(c.deleteBranch('default', present(dev))).rejects.toMatchObject({
      reason: 'branch_has_children',
    });
  });

  it('copies roles and databases to a child branch', async () => {
    const c = createMockPgControl();
    const { branch } = await c.createBranch('default', 'prj-shop', {
      name: 'child',
      parentId: 'br-main',
      protected: false,
    });
    const roles = await c.listRoles('default', 'prj-shop', branch.id);
    expect(roles.map((r) => r.name).sort()).toEqual(['app', 'loams_admin']);
  });

  it('refuses a database whose owner is not on the branch', async () => {
    const c = createMockPgControl();
    await expect(
      c.createDatabase('default', 'prj-shop', 'br-main', 'x', 'ghost'),
    ).rejects.toMatchObject({ reason: 'owner_role_not_found' });
  });

  it('moves a project create from pending to succeeded and makes main', async () => {
    const c = createMockPgControl();
    const { project, operation } = await c.createProject('default', {
      name: 'fresh',
      pgVersion: 17,
    });
    expect(operation.state).toBe('pending');
    let state = operation.state;
    for (let i = 0; i < 6 && state !== 'succeeded'; i++) {
      state = present((await c.listOperations('default')).find((o) => o.id === operation.id)).state;
    }
    expect(state).toBe('succeeded');
    const branches = await c.listBranches('default', project.id);
    expect(branches.map((b) => [b.name, b.isDefault])).toEqual([['main', true]]);
  });
});
