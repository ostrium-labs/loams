// A fixture-backed, in-memory `PgControlClient`. It stands in for loams.postgres.v1 until
// @loams/proto generates it and the engine serves it. It keeps the server's rules that the
// page relies on (PG2 Tasks 5-7): creates return an operation that a reconciler moves from
// pending through running to succeeded; a branch with children, or the default branch,
// cannot be deleted; names are unique per scope; a database's owner must be a role on the
// branch; a child branch inherits its parent's roles and databases; a role's password is
// returned once and never kept. Each `listOperations` call advances the reconciler one step,
// so the page's polling drives it and tests are deterministic.

import { fixtureState, type MockState } from './fixtures.js';
import {
  type CreateBranchInput,
  type CreateProjectInput,
  type PgVersionInfo,
  type IssuedPassword,
  type PgBranch,
  type PgControlClient,
  PgControlError,
  type PgOperation,
  type PgProject,
  TERMINAL_OPERATION_STATES,
} from './types.js';

const NAME = /^[a-z_][a-z0-9_-]{0,62}$/;

export interface MockOptions {
  /** The starting records; the fixtures by default. */
  state?: MockState;
  /** The clock, for tests. */
  now?: () => Date;
  /** Makes a password; random by default. */
  password?: () => string;
}

function randomPassword(): string {
  const bytes = new Uint8Array(32);
  globalThis.crypto.getRandomValues(bytes);
  let s = '';
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** The mock's majors: design 62 §7.1's "18 beta" row. */
export const MOCK_PG_VERSIONS: PgVersionInfo[] = [
  { major: 18, state: 'beta', isDefault: false, minor: '18.6', eolDate: '2030-11-14' },
  { major: 17, state: 'supported', isDefault: true, minor: '17.11', eolDate: '2029-11-08' },
  { major: 16, state: 'supported', isDefault: false, minor: '16.15', eolDate: '2028-11-09' },
];

export function createMockPgControl(opts: MockOptions = {}): PgControlClient {
  const st: MockState = structuredClone(opts.state ?? fixtureState());
  const now = () => (opts.now ? opts.now() : new Date()).toISOString();
  const makePassword = opts.password ?? randomPassword;
  let seq = 0;
  const nextId = (prefix: string) => `${prefix}-${(++seq).toString(36).padStart(4, '0')}`;
  // Operation id -> what finishing it does.
  const finish = new Map<string, () => void>();

  const project = (ns: string, id: string): PgProject => {
    const p = st.projects.find((x) => x.namespace === ns && x.id === id);
    if (!p) throw new PgControlError('not_found', `project ${id} not found`);
    return p;
  };
  const branch = (ns: string, projectId: string, id: string): PgBranch => {
    project(ns, projectId);
    const b = st.branches.find((x) => x.projectId === projectId && x.id === id);
    if (!b) throw new PgControlError('not_found', `branch ${id} not found`);
    return b;
  };
  const checkName = (name: string) => {
    if (!NAME.test(name)) {
      throw new PgControlError(
        'invalid_name',
        'a name is 1-63 lowercase letters, digits, "_" or "-", starting with a letter or "_"',
      );
    }
  };
  const operation = (
    ns: string,
    kind: string,
    target: Record<string, string>,
    total: number,
    unit: string,
    done: () => void,
  ): PgOperation => {
    const t = now();
    const op: PgOperation = {
      id: nextId('op'),
      kind,
      namespace: ns,
      target,
      state: 'pending',
      progress: { done: 0, total, unit, phase: 'queued' },
      createdAt: t,
      updatedAt: t,
    };
    st.operations.unshift(op);
    finish.set(op.id, done);
    return structuredClone(op);
  };
  // One reconciler pass: every open operation moves one step.
  const step = () => {
    for (const op of st.operations) {
      if (TERMINAL_OPERATION_STATES.includes(op.state) || op.state === 'awaiting_approval') {
        continue;
      }
      const p = op.progress ?? { done: 0, total: 1, unit: 'steps', phase: '' };
      if (op.state === 'pending') {
        op.state = 'running';
        op.progress = { ...p, phase: 'reconciling' };
      } else if (p.done + 1 < p.total) {
        op.progress = { ...p, done: p.done + 1 };
      } else {
        op.state = 'succeeded';
        op.progress = { ...p, done: p.total, phase: 'done' };
        finish.get(op.id)?.();
        finish.delete(op.id);
      }
      op.updatedAt = now();
    }
  };

  const copyRolesAndDatabases = (fromBranch: string, toBranch: string) => {
    const t = now();
    for (const r of st.roles.filter((x) => x.branchId === fromBranch)) {
      st.roles.push({ ...r, branchId: toBranch, createTime: t });
    }
    for (const d of st.databases.filter((x) => x.branchId === fromBranch)) {
      st.databases.push({ ...d, branchId: toBranch, createTime: t });
    }
  };

  const issue = (branchId: string, name: string) => {
    const role = st.roles.find((r) => r.branchId === branchId && r.name === name);
    if (!role) throw new PgControlError('not_found', `role ${name} not found on this branch`);
    if (role.system) {
      throw new PgControlError('system_role', `${name} is a system role`);
    }
    // The password is handed back and dropped: the mock, like the page, never keeps it.
    return { role: structuredClone(role), password: makePassword() } satisfies IssuedPassword;
  };

  const client: PgControlClient = {
    mock: true,

    async listProjects(ns) {
      return structuredClone(st.projects.filter((p) => p.namespace === ns));
    },

    async listPgVersions() {
      return structuredClone(MOCK_PG_VERSIONS);
    },

    async createProject(ns, input: CreateProjectInput) {
      const name = input.name.trim();
      checkName(name);
      const v = MOCK_PG_VERSIONS.find((m) => m.major === input.pgVersion);
      if (!v || v.state === 'maintenance') {
        throw new PgControlError(
          'unsupported_pg_version',
          `Postgres ${input.pgVersion} is not offered`,
        );
      }
      if (v.state === 'beta' && !input.allowBetaMajor) {
        throw new PgControlError(
          'beta_major_requires_opt_in',
          `Postgres ${input.pgVersion} is in beta: set allow_beta_major to use it`,
        );
      }
      if (st.projects.some((p) => p.namespace === ns && p.name === name)) {
        throw new PgControlError('already_exists', `a project named ${name} exists`);
      }
      const t = now();
      const id = nextId('prj');
      const mainId = nextId('br');
      const p: PgProject = {
        id,
        namespace: ns,
        name,
        pgVersion: input.pgVersion,
        region: input.region ?? 'local',
        historyRetentionSeconds: 7 * 24 * 3600,
        defaultBranchId: mainId,
        walPool: 'default',
        state: 'creating',
        createTime: t,
        updateTime: t,
        version: 1,
      };
      st.projects.push(p);
      const op = operation(ns, 'postgres.project.create', { project: id }, 3, 'steps', () => {
        p.state = 'ready';
        p.updateTime = now();
        p.version += 1;
        st.branches.push({
          id: mainId,
          projectId: id,
          name: 'main',
          parentId: '',
          parentLsn: '',
          isDefault: true,
          protected: false,
          state: 'ready',
          wal: {
            commitLsn: '0/14EE6B8',
            flushLsn: '0/14EE6B8',
            remoteConsistentLsn: '0/14EE6B8',
            backupLsn: '0/14EE6B8',
          },
          logicalSizeBytes: 0,
          createTime: now(),
          updateTime: now(),
          version: 1,
        });
        st.roles.push({ name: 'loams_admin', branchId: mainId, system: true, createTime: now() });
        st.databases.push({
          name: 'postgres',
          branchId: mainId,
          ownerRole: 'loams_admin',
          createTime: now(),
        });
      });
      return { project: structuredClone(p), operation: op };
    },

    async deleteProject(ns, p) {
      const rec = project(ns, p.id);
      if (rec.version !== p.version) {
        throw new PgControlError('version_mismatch', 'the project changed; reload and try again');
      }
      rec.state = 'deleting';
      rec.version += 1;
      return operation(ns, 'postgres.project.delete', { project: rec.id }, 2, 'steps', () => {
        const branchIds = new Set(
          st.branches.filter((b) => b.projectId === rec.id).map((b) => b.id),
        );
        st.branches = st.branches.filter((b) => b.projectId !== rec.id);
        st.roles = st.roles.filter((r) => !branchIds.has(r.branchId));
        st.databases = st.databases.filter((d) => !branchIds.has(d.branchId));
        st.projects = st.projects.filter((x) => x.id !== rec.id);
      });
    },

    async listBranches(ns, projectId) {
      project(ns, projectId);
      return structuredClone(st.branches.filter((b) => b.projectId === projectId));
    },

    async createBranch(ns, projectId, input: CreateBranchInput) {
      const p = project(ns, projectId);
      const parent = branch(ns, projectId, input.parentId);
      const name = input.name.trim();
      checkName(name);
      if (st.branches.some((b) => b.projectId === projectId && b.name === name)) {
        throw new PgControlError('already_exists', `a branch named ${name} exists`);
      }
      const t = now();
      const point = input.point;
      const b: PgBranch = {
        id: nextId('br'),
        projectId: p.id,
        name,
        parentId: parent.id,
        // A timestamp resolves to an LSN on the server; the mock uses the parent's head.
        parentLsn: point && 'lsn' in point ? point.lsn : (parent.wal?.commitLsn ?? '0/0'),
        parentTime: point && 'time' in point ? point.time : t,
        isDefault: false,
        protected: input.protected,
        state: 'creating',
        wal: parent.wal ? { ...parent.wal } : undefined,
        logicalSizeBytes: parent.logicalSizeBytes,
        createTime: t,
        updateTime: t,
        version: 1,
      };
      st.branches.push(b);
      copyRolesAndDatabases(parent.id, b.id);
      const op = operation(
        ns,
        'postgres.branch.create',
        { project: p.id, branch: b.id },
        2,
        'steps',
        () => {
          b.state = 'ready';
          b.updateTime = now();
          b.version += 1;
        },
      );
      return { branch: structuredClone(b), operation: op };
    },

    async setBranchProtected(ns, b, value) {
      const rec = branch(ns, b.projectId, b.id);
      if (rec.version !== b.version) {
        throw new PgControlError('version_mismatch', 'the branch changed; reload and try again');
      }
      rec.protected = value;
      rec.version += 1;
      rec.updateTime = now();
      return structuredClone(rec);
    },

    async deleteBranch(ns, b) {
      const rec = branch(ns, b.projectId, b.id);
      if (rec.isDefault) {
        throw new PgControlError('default_branch', 'the default branch cannot be deleted');
      }
      if (st.branches.some((x) => x.parentId === rec.id)) {
        throw new PgControlError('branch_has_children', `${rec.name} has child branches`);
      }
      if (rec.version !== b.version) {
        throw new PgControlError('version_mismatch', 'the branch changed; reload and try again');
      }
      rec.state = 'deleting';
      rec.version += 1;
      return operation(
        ns,
        'postgres.branch.delete',
        { project: rec.projectId, branch: rec.id },
        2,
        'steps',
        () => {
          st.branches = st.branches.filter((x) => x.id !== rec.id);
          st.roles = st.roles.filter((r) => r.branchId !== rec.id);
          st.databases = st.databases.filter((d) => d.branchId !== rec.id);
        },
      );
    },

    async listRoles(ns, projectId, branchId) {
      branch(ns, projectId, branchId);
      return structuredClone(st.roles.filter((r) => r.branchId === branchId));
    },

    async createRole(ns, projectId, branchId, name) {
      branch(ns, projectId, branchId);
      const n = name.trim();
      checkName(n);
      if (st.roles.some((r) => r.branchId === branchId && r.name === n)) {
        throw new PgControlError('already_exists', `a role named ${n} exists on this branch`);
      }
      st.roles.push({ name: n, branchId, system: false, createTime: now() });
      return issue(branchId, n);
    },

    async resetRolePassword(ns, projectId, branchId, name) {
      branch(ns, projectId, branchId);
      return issue(branchId, name);
    },

    async deleteRole(ns, projectId, branchId, name) {
      branch(ns, projectId, branchId);
      const role = st.roles.find((r) => r.branchId === branchId && r.name === name);
      if (!role) throw new PgControlError('not_found', `role ${name} not found`);
      if (role.system) throw new PgControlError('system_role', `${name} is a system role`);
      if (st.databases.some((d) => d.branchId === branchId && d.ownerRole === name)) {
        throw new PgControlError('role_owns_database', `${name} owns a database on this branch`);
      }
      st.roles = st.roles.filter((r) => r !== role);
    },

    async listDatabases(ns, projectId, branchId) {
      branch(ns, projectId, branchId);
      return structuredClone(st.databases.filter((d) => d.branchId === branchId));
    },

    async createDatabase(ns, projectId, branchId, name, ownerRole) {
      branch(ns, projectId, branchId);
      const n = name.trim();
      checkName(n);
      if (!st.roles.some((r) => r.branchId === branchId && r.name === ownerRole)) {
        throw new PgControlError('owner_role_not_found', `role ${ownerRole} is not on this branch`);
      }
      if (st.databases.some((d) => d.branchId === branchId && d.name === n)) {
        throw new PgControlError('already_exists', `a database named ${n} exists on this branch`);
      }
      const d = { name: n, branchId, ownerRole, createTime: now() };
      st.databases.push(d);
      return structuredClone(d);
    },

    async deleteDatabase(ns, projectId, branchId, name) {
      branch(ns, projectId, branchId);
      const before = st.databases.length;
      st.databases = st.databases.filter((d) => !(d.branchId === branchId && d.name === name));
      if (st.databases.length === before) {
        throw new PgControlError('not_found', `database ${name} not found`);
      }
    },

    async listOperations(ns) {
      step();
      return structuredClone(st.operations.filter((o) => o.namespace === ns));
    },
  };
  return client;
}
