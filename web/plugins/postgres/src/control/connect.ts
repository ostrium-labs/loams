// `PgControlClient` over Connect's JSON protocol, for `loams pg-control`'s own API listener
// (PG2 R9.1): loams.postgres.v1.PostgresService and loams.operations.v1's GetOperation and
// ListOperations. @loams/proto does not generate loams.postgres.v1 yet, so the calls are plain
// JSON posts (proto3 JSON: camelCase fields, enums by name, 64-bit integers as strings,
// durations as "<seconds>s"). In the desktop the base is `loams-app://console/pg-control/`,
// which Electron main forwards to pg-control with the API key only main holds.

import {
  type BranchState,
  type CreateBranchInput,
  type CreateProjectInput,
  type PgVersionInfo,
  type IssuedPassword,
  type OperationState,
  type PgBranch,
  type PgControlClient,
  PgControlError,
  type PgDatabase,
  type PgOperation,
  type PgProject,
  type PgRole,
  type ProjectState,
} from './types.js';

const POSTGRES = 'loams.postgres.v1.PostgresService';
const OPERATIONS = 'loams.operations.v1.OperationsService';

/** Main's marker on its own answer when pg-control did not answer (desktop protocol route). */
export const PG_CONTROL_STATUS_HEADER = 'x-loams-pg-control';

/** pg-control could not be reached: there is no control plane to show. */
export class PgControlUnreachable extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'PgControlUnreachable';
  }
}

export interface ConnectPgControlOptions {
  /** Ends with `/`; the service path is appended, `<base>loams.postgres.v1.PostgresService/…`. */
  baseUrl: string;
  fetch?: typeof globalThis.fetch;
  /** Makes an `idempotency_key`; random by default. */
  idempotencyKey?: () => string;
  /**
   * pg-control's address as the page shows it, when `baseUrl` is a proxy (the desktop's
   * `loams-app://console/pg-control/`); the base URL's host by default.
   */
  endpoint?: string;
}

type Json = Record<string, unknown>;

const str = (v: unknown): string => (typeof v === 'string' ? v : '');
const num = (v: unknown): number => {
  const n = typeof v === 'number' ? v : typeof v === 'string' ? Number(v) : 0;
  return Number.isFinite(n) ? n : 0;
};
const opt = (v: unknown): string | undefined => (typeof v === 'string' && v ? v : undefined);
const obj = (v: unknown): Json => (v && typeof v === 'object' ? (v as Json) : {});
const list = (v: unknown): Json[] => (Array.isArray(v) ? v.map(obj) : []);
/** google.protobuf.Duration in JSON: `"604800s"`, `"1.5s"`. */
const seconds = (v: unknown): number => (typeof v === 'string' ? Number.parseFloat(v) || 0 : 0);

/** An enum by name (`PROJECT_STATE_READY` → `ready`), or by number when the name is unknown. */
function enumOf<T extends string>(v: unknown, prefix: string, byNumber: readonly T[], dflt: T): T {
  if (typeof v === 'number') return byNumber[v] ?? dflt;
  if (typeof v !== 'string' || !v.startsWith(prefix)) return dflt;
  const name = v.slice(prefix.length).toLowerCase() as T;
  return byNumber.includes(name) ? name : dflt;
}

// Index = the proto enum number; 0 (UNSPECIFIED) reads as the first state.
const PROJECT_STATES = [
  'creating',
  'creating',
  'ready',
  'upgrading',
  'deleting',
  'failed',
] as const satisfies readonly ProjectState[];
const BRANCH_STATES = [
  'creating',
  'creating',
  'ready',
  'restoring',
  'deleting',
  'failed',
] as const satisfies readonly BranchState[];
const OPERATION_STATES = [
  'pending',
  'pending',
  'awaiting_approval',
  'running',
  'succeeded',
  'failed',
  'canceled',
] as const satisfies readonly OperationState[];

export function projectFromJson(j: Json): PgProject {
  return {
    id: str(j.id),
    namespace: str(j.namespace),
    name: str(j.name),
    pgVersion: num(j.pgVersion),
    region: str(j.region),
    historyRetentionSeconds: seconds(j.historyRetention),
    defaultBranchId: str(j.defaultBranchId),
    walPool: str(j.walPool),
    state: enumOf(j.state, 'PROJECT_STATE_', PROJECT_STATES, 'creating'),
    createTime: str(j.createTime),
    updateTime: str(j.updateTime),
    version: num(j.version),
  };
}

export function branchFromJson(j: Json): PgBranch {
  const wal = j.wal ? obj(j.wal) : undefined;
  return {
    id: str(j.id),
    projectId: str(j.projectId),
    name: str(j.name),
    parentId: str(j.parentId),
    parentLsn: str(j.parentLsn),
    parentTime: opt(j.parentTime),
    isDefault: j.isDefault === true,
    protected: j.protected === true,
    expireTime: opt(j.expireTime),
    state: enumOf(j.state, 'BRANCH_STATE_', BRANCH_STATES, 'creating'),
    wal: wal && {
      commitLsn: str(wal.commitLsn),
      flushLsn: str(wal.flushLsn),
      remoteConsistentLsn: str(wal.remoteConsistentLsn),
      backupLsn: str(wal.backupLsn),
    },
    logicalSizeBytes: num(j.logicalSizeBytes),
    createTime: str(j.createTime),
    updateTime: str(j.updateTime),
    version: num(j.version),
  };
}

const roleFromJson = (j: Json): PgRole => ({
  name: str(j.name),
  branchId: str(j.branchId),
  system: j.system === true,
  createTime: str(j.createTime),
});

const databaseFromJson = (j: Json): PgDatabase => ({
  name: str(j.name),
  branchId: str(j.branchId),
  ownerRole: str(j.ownerRole),
  createTime: str(j.createTime),
});

export function operationFromJson(j: Json): PgOperation {
  const target: Record<string, string> = {};
  for (const [k, v] of Object.entries(obj(j.target))) target[k] = str(v);
  const progress = j.progress ? obj(j.progress) : undefined;
  const error = j.error ? obj(j.error) : undefined;
  return {
    id: str(j.id),
    kind: str(j.kind),
    namespace: str(j.namespace),
    target,
    state: enumOf(j.state, 'OPERATION_STATE_', OPERATION_STATES, 'pending'),
    progress: progress && {
      done: num(progress.done),
      total: num(progress.total),
      unit: str(progress.unit),
      phase: str(progress.phase),
    },
    createdAt: str(j.createdAt),
    updatedAt: str(j.updatedAt),
    error: error && { code: str(error.code), message: str(error.message) },
  };
}

/** Reads one length-delimited string field from protobuf bytes (ErrorInfo.reason is field 1). */
function stringField(bytes: Uint8Array, field: number): string | undefined {
  let i = 0;
  const varint = (): number => {
    let out = 0;
    let shift = 0;
    while (i < bytes.length) {
      const b = bytes[i++] as number;
      out += (b & 0x7f) * 2 ** shift;
      if (b < 0x80) return out;
      shift += 7;
    }
    throw new Error('truncated');
  };
  try {
    while (i < bytes.length) {
      const key = varint();
      const wire = key & 7;
      if (wire === 0) varint();
      else if (wire === 1) i += 8;
      else if (wire === 5) i += 4;
      else if (wire === 2) {
        const len = varint();
        if (key >>> 3 === field) return new TextDecoder().decode(bytes.subarray(i, i + len));
        i += len;
      } else return undefined;
    }
  } catch {
    return undefined;
  }
  return undefined;
}

const b64 = (s: string): Uint8Array => {
  const bin = atob(s.replace(/-/g, '+').replace(/_/g, '/'));
  const out = new Uint8Array(bin.length);
  for (let k = 0; k < bin.length; k++) out[k] = bin.charCodeAt(k);
  return out;
};

/** A Connect JSON error body: `ErrorInfo.reason` from its details, else the code. */
export function errorFromJson(body: unknown, status: number): PgControlError {
  const j = obj(body);
  const code = str(j.code) || `http_${status}`;
  const message = str(j.message) || `the control plane answered ${status}`;
  for (const d of list(j.details)) {
    if (!str(d.type).endsWith('loams.errors.v1.ErrorInfo')) continue;
    const debug = obj(d.debug);
    const reason =
      str(debug.reason) ||
      (() => {
        try {
          return stringField(b64(str(d.value)), 1);
        } catch {
          return undefined;
        }
      })();
    if (reason) return new PgControlError(reason, message);
  }
  return new PgControlError(code, message);
}

const randomKey = () =>
  globalThis.crypto?.randomUUID?.() ?? `k-${Date.now().toString(36)}-${Math.random()}`;

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export function createConnectPgControl(options: ConnectPgControlOptions): PgControlClient {
  const base = options.baseUrl.endsWith('/') ? options.baseUrl : `${options.baseUrl}/`;
  const doFetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
  const key = options.idempotencyKey ?? randomKey;

  async function call(service: string, method: string, body: Json): Promise<Json> {
    let res: Response;
    try {
      res = await doFetch(`${base}${service}/${method}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'connect-protocol-version': '1' },
        body: JSON.stringify(body),
      });
    } catch (e) {
      throw new PgControlUnreachable(e instanceof Error ? e.message : String(e));
    }
    const text = await res.text();
    let parsed: unknown;
    try {
      parsed = text ? JSON.parse(text) : {};
    } catch {
      parsed = { message: text };
    }
    if (res.headers.get(PG_CONTROL_STATUS_HEADER) === 'unreachable')
      throw new PgControlUnreachable(str(obj(parsed).message) || 'pg-control did not answer');
    if (!res.ok) throw errorFromJson(parsed, res.status);
    return obj(parsed);
  }

  const pg = (method: string, body: Json) => call(POSTGRES, method, body);

  /** Every page of a List call. */
  async function all(service: string, method: string, body: Json, field: string): Promise<Json[]> {
    const out: Json[] = [];
    let pageToken = '';
    for (let n = 0; n < 100; n++) {
      const r = await call(service, method, { ...body, pageSize: 100, pageToken });
      out.push(...list(r[field]));
      pageToken = str(r.nextPageToken);
      if (!pageToken) break;
    }
    return out;
  }

  const scope = (namespace: string, projectId: string, branchId: string) => ({
    namespace,
    projectId,
    branchId,
  });

  return {
    endpoint: options.endpoint ?? hostOf(options.baseUrl),
    async listProjects(namespace) {
      return (await all(POSTGRES, 'ListProjects', { namespace }, 'projects')).map(projectFromJson);
    },
    async listPgVersions() {
      const r = await pg('ListPgVersions', {});
      return list(r.versions).map((v): PgVersionInfo => {
        const j = obj(v);
        const state = String(j.state ?? '');
        return {
          major: num(j.major),
          state: state.endsWith('BETA')
            ? 'beta'
            : state.endsWith('MAINTENANCE')
              ? 'maintenance'
              : 'supported',
          isDefault: j.isDefault === true,
          minor: String(j.minor ?? ''),
          eolDate: String(j.eolDate ?? ''),
        };
      });
    },
    async createProject(namespace, input: CreateProjectInput) {
      const r = await pg('CreateProject', {
        namespace,
        name: input.name,
        pgVersion: input.pgVersion,
        ...(input.region ? { region: input.region } : {}),
        ...(input.allowBetaMajor ? { allowBetaMajor: true } : {}),
        idempotencyKey: key(),
      });
      return {
        project: projectFromJson(obj(r.project)),
        operation: operationFromJson(obj(r.operation)),
      };
    },
    async deleteProject(namespace, project) {
      const r = await pg('DeleteProject', {
        namespace,
        projectId: project.id,
        expectedVersion: String(project.version),
        idempotencyKey: key(),
      });
      return operationFromJson(obj(r.operation));
    },

    async listBranches(namespace, projectId) {
      return (await all(POSTGRES, 'ListBranches', { namespace, projectId }, 'branches')).map(
        branchFromJson,
      );
    },
    async createBranch(namespace, projectId, input: CreateBranchInput) {
      const point = input.point
        ? 'lsn' in input.point
          ? { lsn: input.point.lsn }
          : { time: input.point.time }
        : {};
      const r = await pg('CreateBranch', {
        namespace,
        projectId,
        name: input.name,
        parentId: input.parentId,
        ...point,
        protected: input.protected,
        idempotencyKey: key(),
      });
      return {
        branch: branchFromJson(obj(r.branch)),
        operation: operationFromJson(obj(r.operation)),
      };
    },
    async setBranchProtected(namespace, branch, value) {
      const r = await pg('UpdateBranch', {
        namespace,
        projectId: branch.projectId,
        branch: { id: branch.id, projectId: branch.projectId, protected: value },
        updateMask: 'protected',
        expectedVersion: String(branch.version),
        idempotencyKey: key(),
      });
      return branchFromJson(obj(r.branch));
    },
    async deleteBranch(namespace, branch) {
      const r = await pg('DeleteBranch', {
        namespace,
        projectId: branch.projectId,
        branchId: branch.id,
        expectedVersion: String(branch.version),
        idempotencyKey: key(),
      });
      return operationFromJson(obj(r.operation));
    },

    async listRoles(namespace, projectId, branchId) {
      return (await all(POSTGRES, 'ListRoles', scope(namespace, projectId, branchId), 'roles')).map(
        roleFromJson,
      );
    },
    async createRole(namespace, projectId, branchId, name): Promise<IssuedPassword> {
      const r = await pg('CreateRole', {
        ...scope(namespace, projectId, branchId),
        name,
        idempotencyKey: key(),
      });
      return { role: roleFromJson(obj(r.role)), password: str(r.password) };
    },
    async resetRolePassword(namespace, projectId, branchId, name): Promise<IssuedPassword> {
      const r = await pg('ResetRolePassword', {
        ...scope(namespace, projectId, branchId),
        name,
        idempotencyKey: key(),
      });
      return { role: roleFromJson(obj(r.role)), password: str(r.password) };
    },
    async deleteRole(namespace, projectId, branchId, name) {
      await pg('DeleteRole', {
        ...scope(namespace, projectId, branchId),
        name,
        idempotencyKey: key(),
      });
    },

    async listDatabases(namespace, projectId, branchId) {
      return (
        await all(POSTGRES, 'ListDatabases', scope(namespace, projectId, branchId), 'databases')
      ).map(databaseFromJson);
    },
    async createDatabase(namespace, projectId, branchId, name, ownerRole) {
      const r = await pg('CreateDatabase', {
        ...scope(namespace, projectId, branchId),
        name,
        ownerRole,
        idempotencyKey: key(),
      });
      return databaseFromJson(obj(r.database));
    },
    async deleteDatabase(namespace, projectId, branchId, name) {
      await pg('DeleteDatabase', {
        ...scope(namespace, projectId, branchId),
        name,
        idempotencyKey: key(),
      });
    },

    async listOperations(namespace) {
      const r = await call(OPERATIONS, 'ListOperations', { namespace, pageSize: 50 });
      return list(r.operations)
        .map(operationFromJson)
        .sort((a, b) => (a.createdAt < b.createdAt ? 1 : a.createdAt > b.createdAt ? -1 : 0));
    },
  };
}

/**
 * The real client when a control plane answers at `baseUrl`, else `fallback` (the
 * fixture-backed mock, which the page labels as sample data). Any answer from pg-control,
 * a refusal included, means it is there.
 */
export async function resolvePgControl(
  options: ConnectPgControlOptions,
  fallback: () => PgControlClient,
  namespace = 'default',
): Promise<PgControlClient> {
  const client = createConnectPgControl(options);
  try {
    await client.listOperations(namespace);
    return client;
  } catch (e) {
    return e instanceof PgControlUnreachable ? fallback() : client;
  }
}
