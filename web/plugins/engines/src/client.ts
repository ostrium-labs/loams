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
  /** The Loams namespace (a project's environment) the instance belongs to. */
  namespace: string;
  /** The org that owns the namespace: the billing subject is `org_<org>`. */
  org?: string | null;
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

/** One metering window (design 63): integers, as Loams Meter's usage events carry them. */
export interface UsageSample {
  window_start: string;
  window_end: string;
  /** CPU time in milliseconds. */
  cpu_ms: number;
  /** Memory as MiB × milliseconds. */
  memory_mib_ms: number;
  state_bytes: number;
  /** The window was billed from the reserved size, not measured. */
  estimated: boolean;
}

export interface Usage {
  samples: UsageSample[];
  /** `state_bytes` is the latest sample's (a gauge). */
  totals: { cpu_ms: number; memory_mib_ms: number; state_bytes: number };
}

/** CPU milliseconds as vCPU-hours. */
export const vcpuHours = (cpuMs: number) => cpuMs / 3_600_000;
/** MiB-milliseconds as GiB-hours. */
export const gibHours = (mibMs: number) => mibMs / 1024 / 3_600_000;

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
  listNamespaces(): Promise<string[]>;
  list(namespace: string, kind?: EngineKind): Promise<Instance[]>;
  get(namespace: string, id: string): Promise<Instance>;
  create(namespace: string, req: InstanceCreate): Promise<Instance>;
  update(namespace: string, id: string, req: InstanceUpdate): Promise<Instance>;
  remove(namespace: string, id: string): Promise<Instance>;
  suspend(namespace: string, id: string): Promise<Instance>;
  resume(namespace: string, id: string): Promise<Instance>;
  usage(namespace: string, id: string, range?: { from?: string; to?: string }): Promise<Usage>;
  sql(namespace: string, id: string, req: SqlRequest, signal?: AbortSignal): Promise<SqlResult>;
  listSessions(namespace: string, id: string): Promise<SparkSession[]>;
  createSession(
    namespace: string,
    id: string,
    req: { kind: SparkSessionKind; conf?: Record<string, string> },
  ): Promise<SparkSession>;
  closeSession(namespace: string, id: string, session: string): Promise<void>;
  listJobs(namespace: string, id: string): Promise<SparkJob[]>;
  submitJob(namespace: string, id: string, req: SparkJobSubmit): Promise<SparkJob>;
  cancelJob(namespace: string, id: string, job: string): Promise<SparkJob>;
}

const enc = encodeURIComponent;

/** `/v1/namespaces/{ns}/engines[/{id}[/rest]]`. */
export function enginesPath(namespace: string, id?: string, rest?: string): string {
  let p = `/v1/namespaces/${enc(namespace)}/engines`;
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
    async listNamespaces() {
      // Every project's environments name their Loams namespace (console contract).
      try {
        const r = await call<{ projects: { slug: string }[] }>('/api/v1/projects');
        const envs = await Promise.all(
          r.projects.map((p) =>
            call<{ environments: { namespace?: string }[] }>(
              `/api/v1/projects/${enc(p.slug)}/environments`,
            ).then(
              (e) => e.environments.map((x) => x.namespace).filter((n): n is string => !!n),
              () => [] as string[],
            ),
          ),
        );
        const ns = [...new Set(envs.flat())];
        return ns.length > 0 ? ns : ['default'];
      } catch {
        return ['default'];
      }
    },
    async list(namespace, kind) {
      const q = kind ? `?kind=${enc(kind)}` : '';
      return (await call<{ instances: Instance[] }>(`${enginesPath(namespace)}${q}`)).instances;
    },
    get: (namespace, id) => call(enginesPath(namespace, id)),
    create: (namespace, req) => send('POST', enginesPath(namespace), req),
    update: (namespace, id, req) => send('PATCH', enginesPath(namespace, id), req),
    remove: (namespace, id) => send('DELETE', enginesPath(namespace, id)),
    suspend: (namespace, id) => send('POST', enginesPath(namespace, id, 'suspend')),
    resume: (namespace, id) => send('POST', enginesPath(namespace, id, 'resume')),
    usage(namespace, id, range) {
      const q = new URLSearchParams();
      if (range?.from) q.set('from', range.from);
      if (range?.to) q.set('to', range.to);
      const qs = q.toString();
      return call(`${enginesPath(namespace, id, 'usage')}${qs ? `?${qs}` : ''}`);
    },
    sql: (namespace, id, req, signal) =>
      send('POST', enginesPath(namespace, id, 'sql'), req, signal),
    async listSessions(namespace, id) {
      return (await call<{ sessions: SparkSession[] }>(enginesPath(namespace, id, 'sessions')))
        .sessions;
    },
    createSession: (namespace, id, req) =>
      send('POST', enginesPath(namespace, id, 'sessions'), req),
    async closeSession(namespace, id, session) {
      await send('DELETE', enginesPath(namespace, id, `sessions/${enc(session)}`));
    },
    async listJobs(namespace, id) {
      return (await call<{ jobs: SparkJob[] }>(enginesPath(namespace, id, 'jobs'))).jobs;
    },
    submitJob: (namespace, id, req) => send('POST', enginesPath(namespace, id, 'jobs'), req),
    cancelJob: (namespace, id, job) =>
      send('POST', enginesPath(namespace, id, `jobs/${enc(job)}/cancel`)),
  };
}

/** Loams Meter refused the work: the plan's allowance is used up (design 63, HTTP 402). */
export const ENTITLEMENT_EXHAUSTED = 'entitlement_exhausted';

export function isEntitlementExhausted(e: unknown): e is ApiError {
  return e instanceof ApiError && (e.status === 402 || e.code === ENTITLEMENT_EXHAUSTED);
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
