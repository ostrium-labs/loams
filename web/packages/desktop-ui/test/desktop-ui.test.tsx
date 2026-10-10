import type { IpcResult, SqlResult, StackState } from '@loams/desktop/contracts';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ConnectPanel, isWrite, MAX_CELL_CHARS, SqlConsole, StackCard } from '../src/index.js';

afterEach(cleanup);

// jsdom has no <dialog> modal support.
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

function fakeStacks(
  initial: StackState,
  resetAnswer: IpcResult<void> = { ok: true, value: undefined },
) {
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
      reset: async (): Promise<IpcResult<void>> => {
        calls.push('reset');
        return resetAnswer;
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

  it('pg_major_mismatch_offers_the_reset_and_a_declined_one_is_quiet', async () => {
    const mismatch: StackState = {
      phase: 'error',
      code: 'pg_major_mismatch',
      message: 'Your local Postgres data was made with Postgres 16.',
    };
    const f = fakeStacks(mismatch, { ok: false, code: 'cancelled', message: 'reset cancelled' });
    render(<StackCard desktop={f.desktop as never} id="postgres" />);
    expect(await screen.findByText('Local data from another Postgres version')).toBeTruthy();
    expect(screen.getByText(/made with Postgres 16/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Reset local Postgres data' }));
    await waitFor(() => expect(f.calls).toEqual(['reset']));
    expect(screen.queryByText('Could not change the stack')).toBeNull();
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
  it('writes_are_confirmed_by_main_not_by_a_second_dialog', async () => {
    const run = vi.fn(async () => result());
    render(<SqlConsole dialect="postgres" run={run} />);
    fireEvent.change(screen.getByLabelText('SQL editor'), { target: { value: 'DELETE FROM t' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    await waitFor(() => expect(run).toHaveBeenCalledWith('DELETE FROM t'));
    expect(screen.queryByRole('button', { name: 'Run anyway' })).toBeNull();
  });

  it('a_cancelled_confirm_shows_no_error', async () => {
    const run = vi.fn(async () => ({
      ok: false as const,
      code: 'cancelled',
      message: 'cancelled',
    }));
    render(<SqlConsole dialect="postgres" run={run} />);
    fireEvent.change(screen.getByLabelText('SQL editor'), { target: { value: 'DELETE FROM t' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    await waitFor(() => expect(run).toHaveBeenCalled());
    await waitFor(() =>
      expect((screen.getByRole('button', { name: 'Run' }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
    expect(screen.queryByText(/Query failed/)).toBeNull();
  });

  it('runs a plain select without a prompt, on Ctrl and Cmd+Enter', async () => {
    const run = vi.fn(async () => result());
    render(<SqlConsole dialect="postgres" run={run} />);
    const editor = screen.getByLabelText('SQL editor');
    fireEvent.change(editor, { target: { value: 'select 1' } });
    fireEvent.keyDown(editor, { key: 'Enter', metaKey: true });
    await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('2 rows in 5 ms')).toBeTruthy();
  });

  it('truncated_notice', async () => {
    const run = async () => result({ truncated: true });
    render(<SqlConsole dialect="postgres" run={run} />);
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
    render(<SqlConsole dialect="postgres" run={run} />);
    fireEvent.change(screen.getByLabelText('SQL editor'), { target: { value: 'select * from x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(await screen.findByText('relation "x" does not exist')).toBeTruthy();
    expect(screen.getByText(/42P01/)).toBeTruthy();
  });
});

describe('isWrite', () => {
  const pg = (sql: string) => isWrite(sql, 'postgres');
  const my = (sql: string) => isWrite(sql, 'mysql');
  it.each([
    ['select 1', false],
    ['  SHOW TABLES', false],
    ['explain select * from t', false],
    ['with a as (select 1) select * from a', false],
    ['select 1; select 2;', false],
    ["select 'delete' as w -- drop table", false],
    ['select \'it\'\'s\' , "a""b"', false],
    ['select $$ drop table t; $$', false],
    ['select $tag$ ; delete from t ; $tag$', false],
    ["select E'a\\'b; ' as x", false],
    ['select $1, a$b from t', false],
    ['insert into t values (1)', true],
    ['with a as (delete from t returning *) select * from a', true],
    ['select 1; drop table t', true],
    ['select * into copy from t', true],
    ['create table t(a int)', true],
    ['set search_path = x', true],
    ["select E'\\'' ; drop table t; select '", true],
    ['select $$ drop', true],
    ['select $a$ x $b$ ; drop table t', true],
    ["select nextval('s')", true],
    ['select pg_terminate_backend(123)', true],
    ["select setval ('s', 1)", true],
    ['select /* unterminated', true],
  ])('postgres: %s -> %s', (sql, want) => expect(pg(sql)).toBe(want));

  it.each([
    ['select 1', false],
    ['show databases', false],
    ["select 'a''b'", false],
    ['select `delete` from t', false],
    ['select 1 # drop table t', false],
    ['select 1 -- drop table t', false],
    ["select '\\'' ; drop table t; select '", true],
    ["select 'a\\'; drop table t; --'", true],
    ['select "x\\"; drop table t; "', true],
    ['select 1 --drop table t', true],
    ['select /*! drop table t */ 1', true],
    ['select sleep(10)', true],
    ["select get_lock('a', 1)", true],
    ["select * from t into outfile '/x'", true],
    ['delete from t', true],
    ["select 'unterminated", true],
  ])('mysql: %s -> %s', (sql, want) => expect(my(sql)).toBe(want));
});

describe('renderer row cap', () => {
  it('slices to 1,000 rows, forces the notice and clips long cells', async () => {
    const rows = Array.from({ length: 1500 }, (_, i) => [i, i === 0 ? 'x'.repeat(5000) : 'v']);
    const run = async (): Promise<IpcResult<SqlResult>> => ({
      ok: true,
      value: { columns: ['n', 's'], rows, rowCount: 1500, truncated: false, elapsedMs: 1 },
    });
    render(<SqlConsole dialect="postgres" run={run} />);
    fireEvent.change(screen.getByLabelText('SQL editor'), { target: { value: 'select 1' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(await screen.findByText(/truncated at 1,000 rows/i)).toBeTruthy();
    expect(screen.getAllByRole('row')).toHaveLength(1001);
    expect(screen.getByText('1,000 rows in 1 ms')).toBeTruthy();
    const clipped = screen.getByText(/^x+…$/);
    expect(clipped.textContent?.length).toBe(MAX_CELL_CHARS + 1);
  });
});

describe('open logs and reveal races', () => {
  it('wires Open logs to stacks.openLogs when present', async () => {
    const f = fakeStacks({ phase: 'stopped' });
    const openLogs = vi.fn();
    (f.desktop.stacks as { openLogs?: unknown }).openLogs = openLogs;
    render(<StackCard desktop={f.desktop as never} id="postgres" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Open logs' }));
    expect(openLogs).toHaveBeenCalledWith('postgres');
  });

  it('a late reveal after Hide does not show the password', async () => {
    let release: (v: string) => void = () => undefined;
    const reveal = () => new Promise<string>((r) => (release = r));
    render(
      <ConnectPanel
        dialect="mysql"
        connection={async () => ({
          host: 'h',
          port: 1,
          database: 'd',
          user: 'u',
          passwordRef: 'r',
        })}
        revealPassword={reveal}
        copy={async () => undefined}
      />,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Reveal' }));
    fireEvent.click(screen.getByRole('button', { name: 'Reveal' }));
    release('late-secret');
    await waitFor(() => expect(document.body.textContent).toContain('late-secret'));
    fireEvent.click(screen.getByRole('button', { name: 'Hide' }));
    expect(document.body.textContent).not.toContain('late-secret');
  });
});
