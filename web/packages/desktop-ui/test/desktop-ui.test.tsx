import type { IpcResult, SqlResult, StackState } from '@loams/desktop/contracts';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ConnectPanel, isWrite, SqlConsole, StackCard } from '../src/index.js';

afterEach(cleanup);

// jsdom has no <dialog> modal support.
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

function fakeStacks(initial: StackState) {
  let cb: (id: 'postgres', s: StackState) => void = () => undefined;
  const calls: string[] = [];
  const desktop = {
    stacks: {
      state: async () => initial,
      start: async () => {
        calls.push('start');
        return { ok: true as const, value: undefined };
      },
      stop: async () => {
        calls.push('stop');
        return { ok: true as const, value: undefined };
      },
      onState: (f: typeof cb) => {
        cb = f;
        return () => undefined;
      },
    },
    shell: { openExternal: vi.fn(async () => ({ ok: true as const, value: undefined })) },
  };
  return { desktop, calls, push: (s: StackState) => act(() => cb('postgres', s)) };
}

describe('stack card', () => {
  it('stack_card_each_phase', async () => {
    const f = fakeStacks({ phase: 'stopped' });
    render(<StackCard desktop={f.desktop as never} id="postgres" />);
    expect(await screen.findByText('Stopped')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Start' }));
    await waitFor(() => expect(f.calls).toEqual(['start']));

    f.push({ phase: 'starting' });
    expect(await screen.findByText('Starting')).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Start' }) as HTMLButtonElement).disabled).toBe(
      true,
    );

    f.push({
      phase: 'running',
      services: [{ name: 'pageserver', state: 'running', ports: ['127.0.0.1:64000'] }],
    });
    expect(await screen.findByText('Running')).toBeTruthy();
    expect(screen.getByText('pageserver')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(f.calls).toEqual(['start', 'stop']));

    f.push({ phase: 'error', message: 'port 5432 is taken' });
    expect(await screen.findByText('port 5432 is taken')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy();
    // No per-stack log opener: the button is present but disabled.
    expect((screen.getByRole('button', { name: 'Open logs' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it('no_runtime_guidance', async () => {
    const f = fakeStacks({ phase: 'unavailable', reason: 'no_container_runtime' });
    render(<StackCard desktop={f.desktop as never} id="postgres" />);
    expect(await screen.findByText(/no container runtime was found/i)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Install Docker' }));
    fireEvent.click(screen.getByRole('button', { name: 'Install Podman' }));
    const urls = f.desktop.shell.openExternal.mock.calls.map((c) => (c as unknown as string[])[0]);
    expect(urls[0]).toContain('docs.docker.com');
    expect(urls[1]).toContain('podman.io');
    expect(screen.queryByRole('button', { name: 'Start' })).toBeNull();
  });
});

describe('connect panel', () => {
  const conn = {
    host: '127.0.0.1',
    port: 55432,
    database: 'postgres',
    user: 'cloud_admin',
    passwordRef: 'secret:x',
  };
  it('password_hidden_until_reveal', async () => {
    const reveal = vi.fn(async () => 'hunter2-sample');
    const copy = vi.fn(async () => undefined);
    render(
      <ConnectPanel
        dialect="postgres"
        connection={async () => conn}
        revealPassword={reveal}
        copy={copy}
      />,
    );
    await screen.findByText('127.0.0.1');
    expect(document.body.textContent).not.toContain('hunter2-sample');
    expect(reveal).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain('psql "host=127.0.0.1 port=55432');
    fireEvent.click(screen.getByRole('button', { name: 'Reveal' }));
    expect(await screen.findByText('hunter2-sample')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Hide' }));
    await waitFor(() => expect(document.body.textContent).not.toContain('hunter2-sample'));
    fireEvent.click(screen.getByRole('button', { name: 'Copy password' }));
    await waitFor(() => expect(copy).toHaveBeenCalledWith('hunter2-sample'));
    // Copy does not reveal.
    expect(document.body.textContent).not.toContain('hunter2-sample');
  });
});

const result = (over: Partial<SqlResult> = {}): IpcResult<SqlResult> => ({
  ok: true,
  value: { columns: ['n'], rows: [[1], [2]], rowCount: 2, truncated: false, elapsedMs: 5, ...over },
});

describe('sql console', () => {
  it('write_requires_confirm', async () => {
    const run = vi.fn(async () => result());
    render(<SqlConsole run={run} />);
    const editor = screen.getByLabelText('SQL editor');
    fireEvent.change(editor, { target: { value: 'DELETE FROM t' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(run).not.toHaveBeenCalled();
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));
    expect(run).not.toHaveBeenCalled();
    fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true });
    fireEvent.click(await screen.findByRole('button', { name: 'Run anyway' }));
    await waitFor(() => expect(run).toHaveBeenCalledWith('DELETE FROM t'));
  });

  it('runs a plain select without a prompt, on Ctrl and Cmd+Enter', async () => {
    const run = vi.fn(async () => result());
    render(<SqlConsole run={run} />);
    const editor = screen.getByLabelText('SQL editor');
    fireEvent.change(editor, { target: { value: 'select 1' } });
    fireEvent.keyDown(editor, { key: 'Enter', metaKey: true });
    await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('2 rows in 5 ms')).toBeTruthy();
  });

  it('truncated_notice', async () => {
    const run = async () => result({ truncated: true });
    render(<SqlConsole run={run} />);
    fireEvent.change(screen.getByLabelText('SQL editor'), {
      target: { value: 'select * from big' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(await screen.findByText(/truncated at 1,000 rows/i)).toBeTruthy();
  });

  it('shows an error panel', async () => {
    const run = async (): Promise<IpcResult<SqlResult>> => ({
      ok: false,
      code: '42P01',
      message: 'relation "x" does not exist',
    });
    render(<SqlConsole run={run} />);
    fireEvent.change(screen.getByLabelText('SQL editor'), { target: { value: 'select * from x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(await screen.findByText('relation "x" does not exist')).toBeTruthy();
    expect(screen.getByText(/42P01/)).toBeTruthy();
  });
});

describe('isWrite', () => {
  it.each([
    ['select 1', false],
    ['  SHOW TABLES', false],
    ['explain select * from t', false],
    ['with a as (select 1) select * from a', false],
    ['select 1; select 2;', false],
    ["select 'delete' as w -- drop table", false],
    ['insert into t values (1)', true],
    ['with a as (delete from t returning *) select * from a', true],
    ['select 1; drop table t', true],
    ['select * into copy from t', true],
    ['create table t(a int)', true],
    ['set search_path = x', true],
  ])('%s -> %s', (sql, want) => expect(isWrite(sql)).toBe(want));
});
