// API2 Tasks 6 and 7: the console API on Connect. Every service of
// loams.auth.v1 and loams.admin.v1 yields a typed client, the reads are
// NO_SIDE_EFFECTS (Connect may send them as GET), the set-state mutations are
// IDEMPOTENT, and the audit watch keeps AP0 Ruling 3's shape.

import { create, type MessageInitShape } from '@bufbuild/protobuf';
import { Code, ConnectError, createClient, createRouterTransport } from '@connectrpc/connect';
import { describe, expect, it } from 'vite-plus/test';
import {
  AgentService,
  AuditOutcome,
  AuditService,
  CreateKeyRequestSchema,
  KeyService,
  OrgRole,
  OrgService,
  ProjectService,
  type WatchAuditEventsResponseSchema,
} from '../src/admin.js';
import { AuthService } from '../src/gen/loams/auth/v1/auth_pb.js';
import { admin, auth, CONSOLE_API_PACKAGES } from '../src/index.js';

const SERVICES = [AuthService, OrgService, ProjectService, AgentService, KeyService, AuditService];

/** NO_SIDE_EFFECTS and IDEMPOTENT, as protobuf-es numbers MethodIdempotency. */
const READ = 1;
const SET_STATE = 2;

describe('the console API (loams.auth.v1, loams.admin.v1)', () => {
  it('builds a client for every service', () => {
    const transport = createRouterTransport(() => {});
    for (const service of SERVICES) {
      const client = createClient(service, transport);
      expect(Object.keys(service.method).length).toBeGreaterThan(0);
      for (const name of Object.keys(service.method)) {
        expect(typeof (client as Record<string, unknown>)[name]).toBe('function');
      }
    }
  });

  it('names both packages in CONSOLE_API_PACKAGES', () => {
    const packages = new Set(SERVICES.map((s) => s.typeName.split('.').slice(0, -1).join('.')));
    expect([...packages].sort()).toEqual([...CONSOLE_API_PACKAGES].sort());
    // The barrel and the namespace exports are the same modules.
    expect(admin.OrgService).toBe(OrgService);
    expect(auth.AuthService).toBe(AuthService);
  });

  it('has every RPC of design §44 §5.2', () => {
    const rpcs = SERVICES.flatMap((s) =>
      Object.values(s.method).map((m) => `${s.typeName.split('.').pop()}/${m.name}`),
    );
    expect(rpcs.sort()).toEqual(
      [
        'AuthService/ListProviders',
        'AuthService/CompleteSetup',
        'AuthService/GetSession',
        'AuthService/CreateSession',
        'AuthService/DeleteSession',
        'AuthService/DecideConsent',
        'OrgService/GetOrg',
        'OrgService/UpdateOrg',
        'OrgService/ListMembers',
        'OrgService/RemoveMember',
        'OrgService/ListInvitations',
        'OrgService/CreateInvitation',
        'OrgService/ListTeams',
        'OrgService/CreateTeam',
        'OrgService/GetTeam',
        'OrgService/AddTeamMember',
        'ProjectService/ListProjects',
        'ProjectService/CreateProject',
        'ProjectService/GetProject',
        'ProjectService/ListAccess',
        'ProjectService/GrantAccess',
        'ProjectService/ListEnvironments',
        'ProjectService/CreateEnvironment',
        'ProjectService/GetEnvironment',
        'ProjectService/GetEnvironmentActivity',
        'AgentService/ListAgents',
        'AgentService/CreateAgent',
        'AgentService/GetAgent',
        'AgentService/UpdateAgent',
        'AgentService/SuspendAgent',
        'AgentService/ResumeAgent',
        'AgentService/ListAgentTokens',
        'AgentService/ListTrustPolicies',
        'AgentService/CreateTrustPolicy',
        'AgentService/ListServiceAccounts',
        'AgentService/CreateServiceAccount',
        'KeyService/ListKeys',
        'KeyService/CreateKey',
        'KeyService/RevokeKey',
        'KeyService/RevokeToken',
        'AuditService/ListAuditEvents',
        'AuditService/WatchAuditEvents',
      ].sort(),
    );
  });

  it('marks reads NO_SIDE_EFFECTS and set-state mutations IDEMPOTENT', () => {
    for (const service of SERVICES) {
      for (const method of Object.values(service.method)) {
        const read = /^(Get|List|Watch)/.test(method.name);
        if (read) expect(method.idempotency, method.name).toBe(READ);
        else expect(method.idempotency, method.name).not.toBe(READ);
      }
    }
    for (const m of [
      OrgService.method.updateOrg,
      OrgService.method.removeMember,
      OrgService.method.addTeamMember,
      ProjectService.method.grantAccess,
      AgentService.method.suspendAgent,
      AgentService.method.resumeAgent,
      KeyService.method.revokeKey,
      KeyService.method.revokeToken,
      AuthService.method.deleteSession,
    ]) {
      expect(m.idempotency, m.name).toBe(SET_STATE);
    }
    expect(AuditService.method.watchAuditEvents.methodKind).toBe('server_streaming');
    // CreateKey is the one create that is not by name, so it is keyed.
    expect(create(CreateKeyRequestSchema, { idempotencyKey: 'k1' }).idempotencyKey).toBe('k1');
  });

  it('round-trips a session, a key and an audit watch through a router', async () => {
    const transport = createRouterTransport(({ service }) => {
      service(AuthService, {
        getSession: () => {
          throw new ConnectError('sign in to continue', Code.Unauthenticated);
        },
        createSession: (req) => ({
          session: { user: { id: 'usr_01', email: req.email }, role: OrgRole.OWNER },
        }),
      });
      service(KeyService, {
        createKey: (req) => ({
          key: { id: 'key_1', name: req.name, scopes: req.scopes },
          secret: 'loams_mock_secret',
        }),
      });
      service(AuditService, {
        async *watchAuditEvents(): AsyncGenerator<
          MessageInitShape<typeof WatchAuditEventsResponseSchema>
        > {
          yield {
            event: { case: 'snapshot', value: { events: [{ id: 'evt_1' }] } },
            cursor: 'a1',
          };
          yield {
            event: {
              case: 'append',
              value: { id: 'evt_2', outcome: AuditOutcome.SUCCESS },
            },
            cursor: 'a2',
          };
          yield { event: { case: 'heartbeat', value: {} }, cursor: 'a2' };
        },
      });
    });
    const authClient = createClient(AuthService, transport);
    await expect(authClient.getSession({})).rejects.toMatchObject({
      code: Code.Unauthenticated,
    });
    const signedIn = await authClient.createSession({ email: 'a@b.dev', password: 'x' });
    expect(signedIn.session?.role).toBe(OrgRole.OWNER);

    const keys = createClient(KeyService, transport);
    const created = await keys.createKey({
      project: 'p',
      environment: 'e',
      name: 'ci',
      scopes: ['query'],
      idempotencyKey: 'k1',
    });
    expect(created.secret).toBe('loams_mock_secret');
    expect(created.key?.scopes).toEqual(['query']);

    const cases: string[] = [];
    const cursors: string[] = [];
    for await (const msg of createClient(AuditService, transport).watchAuditEvents({})) {
      cases.push(msg.event.case ?? '');
      cursors.push(msg.cursor);
    }
    expect(cases).toEqual(['snapshot', 'append', 'heartbeat']);
    expect(cursors).toEqual(['a1', 'a2', 'a2']);
  });
});
