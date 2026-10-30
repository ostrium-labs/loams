// The Postgres control plane as the page sees it: loams.postgres.v1 (PostgresService) and the
// loams.operations.v1 records its long-running calls return. @loams/proto does not generate
// loams.postgres.v1 yet, so these are hand-written plain shapes that follow the proto field
// for field (camelCase, timestamps as RFC 3339 strings, durations in seconds).
// `createConnectPgControl` implements `PgControlClient` over Connect JSON against pg-control's
// own listener; the page falls back to `createMockPgControl` when no control plane answers.

export type ProjectState = 'creating' | 'ready' | 'upgrading' | 'deleting' | 'failed';
export type BranchState = 'creating' | 'ready' | 'restoring' | 'deleting' | 'failed';
export type OperationState =
  | 'pending'
  | 'awaiting_approval'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'canceled';

/** loams.postgres.v1.Project */
export interface PgProject {
  id: string;
  namespace: string;
  name: string;
  pgVersion: number;
  region: string;
  /** `history_retention`, in seconds. */
  historyRetentionSeconds: number;
  defaultBranchId: string;
  walPool: string;
  state: ProjectState;
  createTime: string;
  updateTime: string;
  version: number;
}

/** loams.postgres.v1.WalHeads */
export interface PgWalHeads {
  commitLsn: string;
  flushLsn: string;
  remoteConsistentLsn: string;
  backupLsn: string;
}

/** loams.postgres.v1.Branch */
export interface PgBranch {
  id: string;
  projectId: string;
  name: string;
  /** Empty for the project's root branch. */
  parentId: string;
  /** The branch point on the parent. */
  parentLsn: string;
  parentTime?: string;
  isDefault: boolean;
  protected: boolean;
  expireTime?: string;
  state: BranchState;
  wal?: PgWalHeads;
  logicalSizeBytes: number;
  createTime: string;
  updateTime: string;
  version: number;
}

/** loams.postgres.v1.Role */
export interface PgRole {
  name: string;
  branchId: string;
  /** A role the platform owns; it cannot be deleted or reset. */
  system: boolean;
  createTime: string;
}

/** loams.postgres.v1.Database */
export interface PgDatabase {
  name: string;
  branchId: string;
  ownerRole: string;
  createTime: string;
}

/** loams.operations.v1.Progress */
export interface PgProgress {
  done: number;
  total: number;
  unit: string;
  phase: string;
}

/** loams.operations.v1.Operation, as far as the page reads it. */
export interface PgOperation {
  id: string;
  kind: string;
  namespace: string;
  target: Record<string, string>;
  state: OperationState;
  progress?: PgProgress;
  createdAt: string;
  updatedAt: string;
  error?: { code: string; message: string };
}

export interface CreateProjectInput {
  name: string;
  pgVersion: number;
  region?: string;
  /** Opts into a beta major (`allow_beta_major`; design 62 §9.2). */
  allowBetaMajor?: boolean;
}

/** One major of `ListPgVersions` (design 62 §9.2). */
export interface PgVersionInfo {
  major: number;
  /** `beta` needs `allowBetaMajor`; `maintenance` is for existing projects only. */
  state: 'supported' | 'beta' | 'maintenance';
  isDefault: boolean;
  minor: string;
  eolDate: string;
}

/** `CreateBranchRequest.point`: the parent's head when absent. */
export type BranchPoint = { lsn: string } | { time: string };

export interface CreateBranchInput {
  name: string;
  parentId: string;
  point?: BranchPoint;
  protected: boolean;
}

/** A role's password. The server returns it once; the page never stores or logs it. */
export interface IssuedPassword {
  role: PgRole;
  password: string;
}

/**
 * A refused call: `loams.errors.v1.ErrorInfo.reason` (for example `branch_has_children`,
 * `already_exists`, `secret_already_issued`) and the server's message.
 */
export class PgControlError extends Error {
  constructor(
    readonly reason: string,
    message: string,
  ) {
    super(message);
    this.name = 'PgControlError';
  }
}

export function failureText(e: unknown): string {
  if (e instanceof PgControlError) return `${e.message} (${e.reason})`;
  return e instanceof Error ? e.message : String(e);
}

/**
 * The calls the page makes. Every mutating call sends a fresh `idempotency_key` and,
 * where the proto has one, the record's `expected_version`; the implementation adds them.
 */
export interface PgControlClient {
  /** Fixture-backed data, not a server: the page says so. */
  readonly mock?: boolean;
  /** Where pg-control answers (host:port), for the page header. */
  readonly endpoint?: string;

  listProjects(namespace: string): Promise<PgProject[]>;
  /** The Postgres majors the control plane serves, newest first. */
  listPgVersions(): Promise<PgVersionInfo[]>;
  createProject(
    namespace: string,
    input: CreateProjectInput,
  ): Promise<{ project: PgProject; operation: PgOperation }>;
  deleteProject(namespace: string, project: PgProject): Promise<PgOperation>;

  listBranches(namespace: string, projectId: string): Promise<PgBranch[]>;
  createBranch(
    namespace: string,
    projectId: string,
    input: CreateBranchInput,
  ): Promise<{ branch: PgBranch; operation: PgOperation }>;
  /** `UpdateBranch` with `update_mask: ["protected"]`. */
  setBranchProtected(namespace: string, branch: PgBranch, value: boolean): Promise<PgBranch>;
  deleteBranch(namespace: string, branch: PgBranch): Promise<PgOperation>;

  listRoles(namespace: string, projectId: string, branchId: string): Promise<PgRole[]>;
  createRole(
    namespace: string,
    projectId: string,
    branchId: string,
    name: string,
  ): Promise<IssuedPassword>;
  resetRolePassword(
    namespace: string,
    projectId: string,
    branchId: string,
    name: string,
  ): Promise<IssuedPassword>;
  deleteRole(namespace: string, projectId: string, branchId: string, name: string): Promise<void>;

  listDatabases(namespace: string, projectId: string, branchId: string): Promise<PgDatabase[]>;
  createDatabase(
    namespace: string,
    projectId: string,
    branchId: string,
    name: string,
    ownerRole: string,
  ): Promise<PgDatabase>;
  deleteDatabase(
    namespace: string,
    projectId: string,
    branchId: string,
    name: string,
  ): Promise<void>;

  /** `OperationsService.ListOperations` for the namespace, newest first. */
  listOperations(namespace: string): Promise<PgOperation[]>;
}

export const TERMINAL_OPERATION_STATES: readonly OperationState[] = [
  'succeeded',
  'failed',
  'canceled',
];
