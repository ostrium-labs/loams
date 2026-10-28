import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import { Code, ConnectError } from '@connectrpc/connect';
import { admin, auth, instance } from '@loams/proto';
import { describe, expect, it } from 'vite-plus/test';
import { type AccountState, createSession, loadAccount } from '../src/index.js';

const account = create(auth.SessionSchema, {
  user: { id: 'usr_01', name: 'Priya Raman', email: 'priya@acme.dev' },
  org: { id: 'org_acme', name: 'Acme Robotics' },
  role: admin.OrgRole.OWNER,
  csrfToken: 'csrf_1',
});

describe('createSession', () => {
  it('selects the first environment and switches on request', () => {
    const at = new Date('2026-10-02T09:00:00Z');
    const session = createSession(
      create(instance.WhoAmIResponseSchema, {
        principal: { id: 'usr_omar', displayName: 'Omar' },
        environments: [
          { id: 'env_dev', name: 'development', namespace: 'search-dev' },
          { id: 'env_prod', name: 'production', namespace: 'search-prod', protected: true },
        ],
        authenticatedAt: timestampFromDate(at),
      }),
    );
    let changes = 0;
    session.subscribe(() => changes++);
    expect(session.principal()?.id).toBe('usr_omar');
    expect(session.environment()?.id).toBe('env_dev');
    session.select('env_prod');
    expect(session.environment()).toEqual({
      id: 'env_prod',
      name: 'production',
      namespace: 'search-prod',
      protected: true,
    });
    expect(changes).toBe(1);
    expect(session.authenticatedAt()?.toISOString()).toBe(at.toISOString());
    expect(() => session.select('env_nope')).toThrow(/unknown environment/);
  });

  it('is empty when signed out', () => {
    const session = createSession(undefined);
    expect(session.principal()).toBeUndefined();
    expect(session.environments()).toEqual([]);
  });

  it('reads_the_console_session_from_get_session', async () => {
    const ok = {
      getSession: async () => create(auth.GetSessionResponseSchema, { session: account }),
    };
    expect(await loadAccount(ok, false)).toEqual({ gate: 'ready', account });
    const signedOut = {
      getSession: async () => {
        throw new ConnectError('no session', Code.Unauthenticated);
      },
    };
    expect(await loadAccount(signedOut, false)).toEqual({ gate: 'signed-out' });
    const down = {
      getSession: async () => {
        throw new ConnectError('down', Code.Unavailable);
      },
    };
    expect(await loadAccount(down, false)).toEqual({ gate: 'none' });
    // Before first-run setup there is nobody to sign in.
    expect(await loadAccount(ok, true)).toEqual({ gate: 'setup' });
  });

  it('a_cookie_session_names_the_person_and_signs_out', async () => {
    let ended = 0;
    let next: AccountState = { gate: 'ready', account };
    const session = createSession(undefined, {
      initial: { gate: 'ready', account },
      reload: async () => next,
      async end() {
        ended++;
      },
    });
    let changes = 0;
    session.subscribe(() => changes++);
    expect(session.gate?.()).toBe('ready');
    expect(session.principal()).toMatchObject({
      id: 'usr_01',
      kind: instance.PrincipalKind.USER,
      displayName: 'Priya Raman',
      email: 'priya@acme.dev',
    });
    await session.signOut?.();
    expect(ended).toBe(1);
    expect(session.gate?.()).toBe('signed-out');
    expect(session.principal()).toBeUndefined();
    next = { gate: 'ready', account };
    await session.refresh?.();
    expect(session.account?.()?.org?.name).toBe('Acme Robotics');
    expect(changes).toBe(2);
  });

  it('has_no_gate_without_loams_auth', () => {
    const session = createSession(undefined);
    expect(session.gate?.()).toBe('none');
    expect(session.signOut).toBeUndefined();
  });
});
