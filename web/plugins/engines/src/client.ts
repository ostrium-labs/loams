// The managed-engines client (design 81, D2015): REST over the active server's
// `platform.fetch`, following api/engines/openapi.json exactly (snake_case JSON). The pages
// talk to the `EnginesClient` interface; `createMockEnginesClient` (mock.ts) backs it with
// sample data until a server serves the engines API.

export type EngineKind = 'risingwave' | 'flink' | 'spark';
export const ENGINE_KINDS: readonly EngineKind[] = ['risingwave', 'flink', 'spark'];

export const KIND_LABELS: Record<EngineKind, string> = {
  risingwave: 'RisingWave',
  flink: 'Flink SQL',
  spark: 'Spark',
};

export type SizeTier = 'xs' | 's' | 'm' | 'l' | 'xl';
export const SIZE_TIERS: readonly SizeTier[] = ['xs', 's', 'm', 'l', 'xl'];

export interface EngineSize {
  tier: SizeTier;
  vcpu_milli: number;
  memory_mib: number;
  replicas: number;
}

export interface SizeRequest {
  tier: SizeTier;
  replicas?: number;
}

export type Phase =
  | 'pending'
  | 'provisioning'
  | 'running'
  | 'scaling'
  | 'upgrading'
  | 'suspending'
  | 'suspended'
  | 'resuming'
  | 'deleting'
  | 'deleted'
  | 'failed';

export type HealthState = 'healthy' | 'degraded' | 'unhealthy' | 'unknown';

export interface HealthCheck {
  name: string;
  state: HealthState;
  message?: string | null;
}

export interface InstanceStatus {
  phase: Phase;
  health: HealthState;
  checks: HealthCheck[];
  version?: string | null;
  message?: string | null;
  observed_at: string;
}

export type EndpointProtocol =
  | 'postgres'
  | 'flink_sql_gateway'
  | 'spark_connect'
  | 'http'
  | 'web_ui'
  | 'kafka';

export interface Endpoint {
  name: string;
  protocol: EndpointProtocol;
  url: string;
  internal: boolean;
}

export type Stability = 'preview' | 'beta' | 'stable';

export interface Instance {
  id: string;
  project: string;
  /** The org that owns the project: the billing subject. */
  org?: string | null;
  environment?: string | null;
  name: string;
  kind: EngineKind;
  /** The requested version; `status.version` is the running one. */
  version?: string | null;
  size: EngineSize;
  config: Record<string, string>;
  labels: Record<string, string>;
  status: InstanceStatus;
  endpoints: Endpoint[];
  state_prefix: string;
  stability: Stability;
  created_at: string;
  updated_at: string;
}

export interface InstanceCreate {
  name: string;
  kind: EngineKind;
  version?: string;
  size: SizeRequest;
  config?: Record<string, string>;
  labels?: Record<string, string>;
  environment?: string;
}

/** Exactly one of `size` (scale) or `version` (upgrade). */
export type InstanceUpdate = { size: SizeRequest } | { version: string };

export type SqlDialect = 'postgres' | 'flink_sql' | 'spark_sql';

export interface EngineInfo {
  kind: EngineKind;
  label: string;
  stability: Stability;
  enabled: boolean;
  versions: string[];
  default_version?: string | null;
  sql_dialect?: SqlDialect | null;
  sessions: boolean;
  jobs: boolean;
  suspend: boolean;
}

export interface SizeInfo {
  tier: SizeTier;
  vcpu_milli: number;
  memory_mib: number;
}

export interface EngineCatalog {
  engines: EngineInfo[];
  sizes: SizeInfo[];
}

export interface UsageSample {
  window_start: string;
  window_end: string;
  vcpu_seconds: number;
  memory_gb_seconds: number;
  state_bytes: number;
}

export interface Usage {
  samples: UsageSample[];
  totals: { vcpu_seconds: number; memory_gb_seconds: number; state_bytes: number };
}

export interface SqlRequest {
  sql: string;
  max_rows?: number;
  timeout_ms?: number;
}

export interface SqlResult {
  columns: { name: string; type: string }[];
  rows: unknown[][];
  row_count: number;
  truncated: boolean;
  elapsed_ms: number;
  notice?: string | null;
}

export type SparkSessionKind = 'sql' | 'python';
export type SparkSessionState = 'starting' | 'idle' | 'busy' | 'closed' | 'failed';

export interface SparkSession {
  id: string;
  kind: SparkSessionKind;
  state: SparkSessionState;
  created_at: string;
  connect_url?: string | null;
}

export type SparkJobKind = 'sql' | 'python' | 'jar';
export type SparkJobState = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled';

export interface SparkJob {
  id: string;
  name: string;
  kind: SparkJobKind;
  state: SparkJobState;
  entrypoint: string;
  args: string[];
  submitted_at: string;
  finished_at?: string | null;
  message?: string | null;
}

export interface SparkJobSubmit {
  name: string;
  kind: SparkJobKind;
  entrypoint: string;
  args?: string[];
  conf?: Record<string, string>;
}

/** An error from a route, `{error, message}`. */
export class ApiError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

export interface EnginesClient {
  /** True when the client serves sample data, not a server's. */
  readonly sample: boolean;
  catalog(): Promise<EngineCatalog>;
  listProjects(): Promise<string[]>;
  list(project: string, kind?: EngineKind): Promise<Instance[]>;
  get(project: string, id: string): Promise<Instance>;
  create(project: string, req: InstanceCreate): Promise<Instance>;
  update(project: string, id: string, req: InstanceUpdate): Promise<Instance>;
  remove(project: string, id: string): Promise<Instance>;
  suspend(project: string, id: string): Promise<Instance>;
  resume(project: string, id: string): Promise<Instance>;
  usage(project: string, id: string, range?: { from?: string; to?: string }): Promise<Usage>;
  sql(project: string, id: string, req: SqlRequest, signal?: AbortSignal): Promise<SqlResult>;
  listSessions(project: string, id: string): Promise<SparkSession[]>;
  createSession(
    project: string,
    id: string,
    req: { kind: SparkSessionKind; conf?: Record<string, string> },
  ): Promise<SparkSession>;
  closeSession(project: string, id: string, session: string): Promise<void>;
  listJobs(project: string, id: string): Promise<SparkJob[]>;
  submitJob(project: string, id: string, req: SparkJobSubmit): Promise<SparkJob>;
  cancelJob(project: string, id: string, job: string): Promise<SparkJob>;
}

const enc = encodeURIComponent;

/** `/v1/projects/{p}/engines[/{id}[/rest]]`. */
export function enginesPath(project: string, id?: string, rest?: string): string {
  let p = `/v1/projects/${enc(project)}/engines`;
  if (id !== undefined) p += `/${enc(id)}`;
  if (rest) p += `/${rest}`;
  return p;
}

export function createEnginesClient(fetchFn: typeof globalThis.fetch, baseUrl = ''): EnginesClient {
  async function call<T>(path: string, init?: RequestInit): Promise<T> {
    const res = await fetchFn(`${baseUrl}${path}`, { ...init, credentials: 'include' });
    if (res.status === 204) return undefined as T;
    const text = await res.text().catch(() => '');
    let body: Record<string, unknown> = {};
    try {
      body = text ? (JSON.parse(text) as Record<string, unknown>) : {};
    } catch {
      // a non-JSON body: the status text stands in
    }
    if (!res.ok) {
      throw new ApiError(
        String(body.error ?? 'error'),
        String(body.message ?? (res.statusText || `HTTP ${res.status}`)),
        res.status,
      );
    }
    return body as T;
  }
  const send = <T>(method: string, path: string, body?: unknown, signal?: AbortSignal) =>
    call<T>(path, {
      method,
      signal,
      ...(body === undefined
        ? {}
        : { headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }),
    });

  return {
    sample: false,
    catalog: () => call<EngineCatalog>('/v1/engines'),
    async listProjects() {
      try {
        const r = await call<{ projects: { slug: string }[] }>('/api/v1/projects');
        const slugs = r.projects.map((p) => p.slug);
        return slugs.length > 0 ? slugs : ['default'];
      } catch {
        return ['default'];
      }
    },
    async list(project, kind) {
      const q = kind ? `?kind=${enc(kind)}` : '';
      return (await call<{ instances: Instance[] }>(`${enginesPath(project)}${q}`)).instances;
    },
    get: (project, id) => call(enginesPath(project, id)),
    create: (project, req) => send('POST', enginesPath(project), req),
    update: (project, id, req) => send('PATCH', enginesPath(project, id), req),
    remove: (project, id) => send('DELETE', enginesPath(project, id)),
    suspend: (project, id) => send('POST', enginesPath(project, id, 'suspend')),
    resume: (project, id) => send('POST', enginesPath(project, id, 'resume')),
    usage(project, id, range) {
      const q = new URLSearchParams();
      if (range?.from) q.set('from', range.from);
      if (range?.to) q.set('to', range.to);
      const qs = q.toString();
      return call(`${enginesPath(project, id, 'usage')}${qs ? `?${qs}` : ''}`);
    },
    sql: (project, id, req, signal) => send('POST', enginesPath(project, id, 'sql'), req, signal),
    async listSessions(project, id) {
      return (await call<{ sessions: SparkSession[] }>(enginesPath(project, id, 'sessions')))
        .sessions;
    },
    createSession: (project, id, req) => send('POST', enginesPath(project, id, 'sessions'), req),
    async closeSession(project, id, session) {
      await send('DELETE', enginesPath(project, id, `sessions/${enc(session)}`));
    },
    async listJobs(project, id) {
      return (await call<{ jobs: SparkJob[] }>(enginesPath(project, id, 'jobs'))).jobs;
    },
    submitJob: (project, id, req) => send('POST', enginesPath(project, id, 'jobs'), req),
    cancelJob: (project, id, job) =>
      send('POST', enginesPath(project, id, `jobs/${enc(job)}/cancel`)),
  };
}

/** A phase in which the instance is changing; the pages poll while one holds. */
export function isTransitional(phase: Phase): boolean {
  return [
    'pending',
    'provisioning',
    'scaling',
    'upgrading',
    'suspending',
    'resuming',
    'deleting',
  ].includes(phase);
}

/** "2 vCPU · 8 GiB × 3". */
export function sizeText(size: EngineSize): string {
  const cpu = `${size.vcpu_milli / 1000}`;
  const mem = size.memory_mib >= 1024 ? `${size.memory_mib / 1024} GiB` : `${size.memory_mib} MiB`;
  return `${size.tier.toUpperCase()} · ${cpu} vCPU · ${mem} × ${size.replicas}`;
}

/** The message to show for a thrown error. */
export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
