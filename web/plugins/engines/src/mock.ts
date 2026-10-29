// A fixture-backed `EnginesClient` (design 81, preview): RisingWave, Flink SQL and Spark
// instances in every phase the pages draw, SQL results, Spark sessions and jobs, and usage
// samples. The mock keeps its changes in memory, so a create, suspend or job submit shows
// on the next read; nothing reaches a server.

import {
  ApiError,
  ENTITLEMENT_EXHAUSTED,
  type EngineCatalog,
  type EngineKind,
  type EngineSize,
  type EnginesClient,
  type Instance,
  type InstanceCreate,
  type SizeRequest,
  type SizeTier,
  type SparkJob,
  type SparkSession,
  type SqlResult,
  type Usage,
} from './client.js';

const SHAPES: Record<SizeTier, { vcpu_milli: number; memory_mib: number }> = {
  xs: { vcpu_milli: 500, memory_mib: 1024 },
  s: { vcpu_milli: 1000, memory_mib: 4096 },
  m: { vcpu_milli: 2000, memory_mib: 8192 },
  l: { vcpu_milli: 4000, memory_mib: 16384 },
  xl: { vcpu_milli: 8000, memory_mib: 32768 },
};

export function sizeOf(req: SizeRequest): EngineSize {
  return { tier: req.tier, ...SHAPES[req.tier], replicas: req.replicas ?? 1 };
}

export const sampleCatalog: EngineCatalog = {
  engines: [
    {
      kind: 'risingwave',
      label: 'RisingWave',
      stability: 'preview',
      enabled: true,
      versions: ['v2.6.1', 'v2.5.3'],
      default_version: 'v2.6.1',
      sql_dialect: 'postgres',
      sessions: false,
      jobs: false,
      suspend: true,
    },
    {
      kind: 'flink',
      label: 'Flink SQL',
      stability: 'preview',
      enabled: true,
      versions: ['2.1.0', '1.20.2'],
      default_version: '2.1.0',
      sql_dialect: 'flink_sql',
      sessions: false,
      jobs: false,
      suspend: true,
    },
    {
      kind: 'spark',
      label: 'Spark',
      stability: 'preview',
      enabled: true,
      versions: ['4.0.1', '3.5.7'],
      default_version: '4.0.1',
      sql_dialect: 'spark_sql',
      sessions: true,
      jobs: true,
      suspend: true,
    },
  ],
  sizes: (Object.keys(SHAPES) as SizeTier[]).map((tier) => ({ tier, ...SHAPES[tier] })),
};

const ago = (ms: number) => new Date(Date.now() - ms).toISOString();
const MIN = 60_000;
const HOUR = 60 * MIN;
const GW = 'gateway.loams.internal';

function instance(
  p: Partial<Instance> & Pick<Instance, 'id' | 'name' | 'kind' | 'status'>,
): Instance {
  return {
    namespace: 'default-dev',
    org: 'acme',
    version: null,
    size: sizeOf({ tier: 's', replicas: 1 }),
    config: {},
    labels: {},
    endpoints: [],
    state_prefix: `engines/${p.namespace ?? 'default-dev'}/${p.id}/`,
    stability: 'preview',
    created_at: ago(3 * 24 * HOUR),
    updated_at: ago(20 * MIN),
    ...p,
  };
}

export function sampleInstances(): Instance[] {
  return [
    instance({
      id: 'eng_rw01orders',
      name: 'orders-stream',
      kind: 'risingwave',
      version: 'v2.6.1',
      size: sizeOf({ tier: 'm', replicas: 2 }),
      labels: { team: 'data' },
      status: {
        phase: 'running',
        health: 'healthy',
        checks: [
          { name: 'meta', state: 'healthy' },
          { name: 'compute', state: 'healthy' },
          { name: 'state store (s3)', state: 'healthy' },
        ],
        version: 'v2.6.1',
        observed_at: ago(15_000),
      },
      endpoints: [
        {
          name: 'sql',
          protocol: 'postgres',
          url: `postgres://${GW}:5432/orders-stream`,
          internal: false,
        },
        {
          name: 'dashboard',
          protocol: 'web_ui',
          url: `https://${GW}/engines/eng_rw01orders/ui`,
          internal: true,
        },
      ],
    }),
    instance({
      id: 'eng_rw02clicks',
      name: 'clicks-agg',
      kind: 'risingwave',
      version: 'v2.5.3',
      status: {
        phase: 'suspended',
        health: 'unknown',
        checks: [],
        version: 'v2.5.3',
        message: 'Suspended; state is kept in object storage.',
        observed_at: ago(2 * HOUR),
      },
    }),
    instance({
      id: 'eng_fl01fraud',
      name: 'fraud-rules',
      kind: 'flink',
      version: '2.1.0',
      size: sizeOf({ tier: 'l', replicas: 3 }),
      status: {
        phase: 'running',
        health: 'degraded',
        checks: [
          { name: 'jobmanager', state: 'healthy' },
          { name: 'taskmanagers', state: 'degraded', message: '2 of 3 TaskManagers ready' },
          { name: 'checkpoints', state: 'healthy' },
        ],
        version: '2.1.0',
        observed_at: ago(30_000),
      },
      endpoints: [
        {
          name: 'sql-gateway',
          protocol: 'flink_sql_gateway',
          url: `https://${GW}/engines/eng_fl01fraud/sql`,
          internal: false,
        },
        {
          name: 'web-ui',
          protocol: 'web_ui',
          url: `https://${GW}/engines/eng_fl01fraud/ui`,
          internal: true,
        },
      ],
    }),
    instance({
      id: 'eng_fl02enrich',
      name: 'enrich',
      kind: 'flink',
      version: '2.1.0',
      created_at: ago(2 * MIN),
      updated_at: ago(2 * MIN),
      status: {
        phase: 'provisioning',
        health: 'unknown',
        checks: [],
        message: 'Starting the JobManager.',
        observed_at: ago(5_000),
      },
    }),
    instance({
      id: 'eng_sp01batch',
      name: 'nightly-batch',
      kind: 'spark',
      version: '4.0.1',
      size: sizeOf({ tier: 'l', replicas: 4 }),
      status: {
        phase: 'running',
        health: 'healthy',
        checks: [
          { name: 'driver', state: 'healthy' },
          { name: 'executors', state: 'healthy' },
        ],
        version: '4.0.1',
        observed_at: ago(10_000),
      },
      endpoints: [
        {
          name: 'connect',
          protocol: 'spark_connect',
          url: `sc://${GW}:15002/;engine=eng_sp01batch`,
          internal: false,
        },
        {
          name: 'spark-ui',
          protocol: 'web_ui',
          url: `https://${GW}/engines/eng_sp01batch/ui`,
          internal: true,
        },
      ],
    }),
    instance({
      id: 'eng_sp02ml',
      name: 'ml-features',
      kind: 'spark',
      version: '3.5.7',
      status: {
        phase: 'failed',
        health: 'unhealthy',
        checks: [{ name: 'driver', state: 'unhealthy', message: 'OOMKilled' }],
        version: '3.5.7',
        message: 'The driver ran out of memory; scale to a larger size and resume.',
        observed_at: ago(40 * MIN),
      },
    }),
  ];
}

export function sampleUsage(kind: EngineKind, phase: string): Usage {
  const samples = Array.from({ length: 12 }, (_, i) => {
    const end = Date.now() - (11 - i) * 5 * MIN;
    const running = phase === 'running';
    const base = kind === 'spark' ? 480 : kind === 'flink' ? 360 : 240;
    return {
      window_start: new Date(end - 5 * MIN).toISOString(),
      window_end: new Date(end).toISOString(),
      // A running instance is measured; a suspended or failed one bills its reserved
      // state only, and the first window of each hour is an estimate (the reserved size).
      cpu_ms: running ? (base + ((i * 37) % 60)) * 1000 : 0,
      memory_mib_ms: running ? (base * 4 + ((i * 53) % 200)) * 1024 * 1000 : 0,
      state_bytes: 1_200_000_000 + i * 15_000_000,
      estimated: running && i % 12 === 0,
    };
  });
  const last = samples[samples.length - 1];
  return {
    samples,
    totals: {
      cpu_ms: samples.reduce((a, s) => a + s.cpu_ms, 0),
      memory_mib_ms: samples.reduce((a, s) => a + s.memory_mib_ms, 0),
      state_bytes: last ? last.state_bytes : 0,
    },
  };
}

function sqlResult(kind: EngineKind, sql: string): SqlResult {
  const q = sql.trim().toLowerCase();
  if (q.startsWith('insert') && kind === 'flink') {
    return {
      columns: [{ name: 'job id', type: 'STRING' }],
      rows: [['a1b2c3d4e5f6']],
      row_count: 1,
      truncated: false,
      elapsed_ms: 412,
      notice: 'Job submitted: a1b2c3d4e5f6 (streaming; it keeps running).',
    };
  }
  if (q.startsWith('create') || q.startsWith('drop') || q.startsWith('insert')) {
    return { columns: [], rows: [], row_count: 0, truncated: false, elapsed_ms: 38 };
  }
  return {
    columns: [
      { name: 'region', type: kind === 'flink' ? 'STRING' : 'varchar' },
      { name: 'orders', type: kind === 'flink' ? 'BIGINT' : 'bigint' },
      { name: 'revenue', type: kind === 'flink' ? 'DECIMAL(18, 2)' : 'numeric' },
    ],
    rows: [
      ['eu-west', 1842, '58210.40'],
      ['us-east', 2930, '91877.15'],
      ['ap-south', 611, '17002.90'],
    ],
    row_count: 3,
    truncated: false,
    elapsed_ms: 24,
  };
}

export interface MockOptions {
  instances?: Instance[];
  namespaces?: string[];
  /**
   * The plan's allowance is used up: create, resume and a scale up answer 402
   * `entitlement_exhausted`, as Loams Meter's gate does (design 63 §17.4).
   */
  exhausted?: boolean;
}

const EXHAUSTED_MESSAGE =
  "engine compute is at 100% of this month's allowance; add credit or wait for the reset";

export function createMockEnginesClient(opts: MockOptions = {}): EnginesClient {
  let instances = opts.instances ?? sampleInstances();
  const namespaces = opts.namespaces ?? ['default-dev', 'default-prod'];
  const gate = () => {
    if (opts.exhausted) throw new ApiError(ENTITLEMENT_EXHAUSTED, EXHAUSTED_MESSAGE, 402);
  };
  const sessions = new Map<string, SparkSession[]>([
    [
      'eng_sp01batch',
      [
        {
          id: 'ses_01',
          kind: 'sql',
          state: 'idle',
          created_at: ago(30 * MIN),
          connect_url: `sc://${GW}:15002/;session_id=ses_01`,
        },
        {
          id: 'ses_02',
          kind: 'python',
          state: 'busy',
          created_at: ago(5 * MIN),
          connect_url: `sc://${GW}:15002/;session_id=ses_02`,
        },
      ],
    ],
  ]);
  const jobs = new Map<string, SparkJob[]>([
    [
      'eng_sp01batch',
      [
        {
          id: 'job_0001',
          name: 'daily-rollup',
          kind: 'sql',
          state: 'succeeded',
          entrypoint: 'INSERT INTO lake.daily SELECT * FROM lake.events_agg',
          args: [],
          submitted_at: ago(9 * HOUR),
          finished_at: ago(8.5 * HOUR),
        },
        {
          id: 'job_0002',
          name: 'compact-events',
          kind: 'python',
          state: 'running',
          entrypoint: 's3://loams-jobs/compact.py',
          args: ['--table', 'lake.events'],
          submitted_at: ago(12 * MIN),
        },
      ],
    ],
  ]);
  let seq = 0;

  const find = (namespace: string, id: string) => {
    const i = instances.find((x) => x.namespace === namespace && x.id === id);
    if (!i)
      throw new ApiError('not_found', `no engine instance ${id} in namespace ${namespace}`, 404);
    return i;
  };
  const patch = async (namespace: string, id: string, f: (i: Instance) => Instance) => {
    const next = f(find(namespace, id));
    const stamped = { ...next, updated_at: new Date().toISOString() };
    instances = instances.map((x) => (x.id === id && x.namespace === namespace ? stamped : x));
    return stamped;
  };
  const phase = (i: Instance, p: Instance['status']['phase'], message?: string): Instance => ({
    ...i,
    status: {
      ...i.status,
      phase: p,
      message: message ?? null,
      observed_at: new Date().toISOString(),
    },
  });
  const spark = (namespace: string, id: string) => {
    const i = find(namespace, id);
    if (i.kind !== 'spark') {
      throw new ApiError('failed_precondition', `${i.name} is not a Spark instance`, 400);
    }
    return i;
  };

  return {
    sample: true,
    catalog: () => Promise.resolve(sampleCatalog),
    listNamespaces: () => Promise.resolve(namespaces),
    list: (namespace, kind) =>
      Promise.resolve(
        instances.filter(
          (i) =>
            i.namespace === namespace && (!kind || i.kind === kind) && i.status.phase !== 'deleted',
        ),
      ),
    get: async (namespace, id) => find(namespace, id),
    async create(namespace, req: InstanceCreate) {
      gate();
      if (!/^[a-z]([a-z0-9-]{0,38}[a-z0-9])?$/.test(req.name)) {
        throw new ApiError('invalid_argument', 'a name is a DNS label of 1 to 40 characters', 400);
      }
      if (instances.some((i) => i.namespace === namespace && i.name === req.name)) {
        throw new ApiError('already_exists', `an instance named ${req.name} exists`, 409);
      }
      seq += 1;
      const info = sampleCatalog.engines.find((e) => e.kind === req.kind);
      const now = new Date().toISOString();
      const created = instance({
        id: `eng_new${String(seq).padStart(4, '0')}`,
        namespace,
        name: req.name,
        kind: req.kind,
        version: req.version ?? info?.default_version ?? null,
        size: sizeOf(req.size),
        config: req.config ?? {},
        labels: req.labels ?? {},
        created_at: now,
        updated_at: now,
        status: { phase: 'provisioning', health: 'unknown', checks: [], observed_at: now },
      });
      instances = [...instances, created];
      return created;
    },
    update: (namespace, id, req) =>
      patch(namespace, id, (i) => {
        if ('size' in req) {
          const next = sizeOf(req.size);
          const up = next.vcpu_milli * next.replicas > i.size.vcpu_milli * i.size.replicas;
          if (up) gate();
          return phase({ ...i, size: next }, 'scaling');
        }
        return phase({ ...i, version: req.version }, 'upgrading');
      }),
    remove: (namespace, id) => patch(namespace, id, (i) => phase(i, 'deleting')),
    suspend: (namespace, id) => patch(namespace, id, (i) => phase(i, 'suspending')),
    resume: (namespace, id) =>
      patch(namespace, id, (i) => {
        gate();
        return phase(i, 'resuming');
      }),
    usage: async (namespace, id) => {
      const i = find(namespace, id);
      return sampleUsage(i.kind, i.status.phase);
    },
    async sql(namespace, id, req) {
      const i = find(namespace, id);
      if (i.kind === 'spark') {
        throw new ApiError('failed_precondition', 'use a Spark session for SQL on Spark', 400);
      }
      if (i.status.phase !== 'running') {
        throw new ApiError('failed_precondition', `${i.name} is ${i.status.phase}`, 400);
      }
      if (/^\s*selec\b/i.test(req.sql)) {
        throw new ApiError('invalid_argument', 'syntax error at or near "selec"', 400);
      }
      return sqlResult(i.kind, req.sql);
    },
    listSessions: async (namespace, id) => {
      spark(namespace, id);
      return sessions.get(id) ?? [];
    },
    async createSession(namespace, id, req) {
      spark(namespace, id);
      seq += 1;
      const s: SparkSession = {
        id: `ses_n${seq}`,
        kind: req.kind,
        state: 'starting',
        created_at: new Date().toISOString(),
        connect_url: `sc://${GW}:15002/;session_id=ses_n${seq}`,
      };
      sessions.set(id, [...(sessions.get(id) ?? []), s]);
      return s;
    },
    async closeSession(namespace, id, session) {
      spark(namespace, id);
      sessions.set(
        id,
        (sessions.get(id) ?? []).map((s) => (s.id === session ? { ...s, state: 'closed' } : s)),
      );
    },
    listJobs: async (namespace, id) => {
      spark(namespace, id);
      return jobs.get(id) ?? [];
    },
    async submitJob(namespace, id, req) {
      spark(namespace, id);
      seq += 1;
      const j: SparkJob = {
        id: `job_n${seq}`,
        name: req.name,
        kind: req.kind,
        state: 'queued',
        entrypoint: req.entrypoint,
        args: req.args ?? [],
        submitted_at: new Date().toISOString(),
      };
      jobs.set(id, [j, ...(jobs.get(id) ?? [])]);
      return j;
    },
    async cancelJob(namespace, id, job) {
      spark(namespace, id);
      const list = jobs.get(id) ?? [];
      const j = list.find((x) => x.id === job);
      if (!j) throw new ApiError('not_found', `no job ${job}`, 404);
      const next: SparkJob = { ...j, state: 'cancelled', finished_at: new Date().toISOString() };
      jobs.set(
        id,
        list.map((x) => (x.id === job ? next : x)),
      );
      return next;
    },
  };
}
