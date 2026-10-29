// `/sql`: the Loams SQL page (§47, SQ1). Databases on the left; the selected database's
// lifecycle (suspend, resume), its branches, the connection panel and an SQL editor on
// the right.

import { PageHead } from '@loams/desktop-ui';
import { Badge, Button, Empty, formatBytes, Notice, SampleDataNotice } from '@loams/ui';
import { useCallback, useEffect, useState } from 'react';
import { Connection } from './Connection.js';
import {
  type Branch,
  canResume,
  canSuspend,
  type Database,
  failureMessage,
  idempotencyKey,
  isTransitional,
  type SqlClient,
} from './client.js';
import { Query } from './Query.js';
import { Stage } from './Stage.js';

/** How often the page reads a database back while it is in a transitional stage. */
export const POLL_MS = 1000;

export interface SqlPageProps {
  client: SqlClient;
  copy: (text: string) => void | Promise<void>;
  namespace?: string;
  /** Marks the page as running against fixtures. */
  mocked?: boolean;
}

type Load<T> =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; value: T };

export function SqlPage({ client, copy, namespace = 'default', mocked }: SqlPageProps) {
  const [dbs, setDbs] = useState<Load<Database[]>>({ state: 'loading' });
  const [selected, setSelected] = useState<string>('');
  const [tick, setTick] = useState(0);

  // oxlint-disable-next-line react/exhaustive-deps -- tick retries on purpose
  useEffect(() => {
    let live = true;
    setDbs({ state: 'loading' });
    client.listDatabases(namespace).then(
      (value) => {
        if (!live) return;
        setDbs({ state: 'ready', value });
        setSelected((s) => (s && value.some((d) => d.id === s) ? s : (value[0]?.id ?? '')));
      },
      (e) => live && setDbs({ state: 'error', message: failureMessage(e) }),
    );
    return () => {
      live = false;
    };
  }, [client, namespace, tick]);

  const replace = useCallback(
    (db: Database) =>
      setDbs((s) =>
        s.state === 'ready'
          ? { state: 'ready', value: s.value.map((d) => (d.id === db.id ? db : d)) }
          : s,
      ),
    [],
  );

  const head = (
    <PageHead
      title="Loams SQL"
      badge={<Badge>loams.sqldb.v1</Badge>}
      subtitle="MySQL-compatible databases on TiDB. Each branch is a copy with its own compute, which suspends when idle."
    />
  );

  if (dbs.state === 'loading')
    return (
      <div className="lc-page">
        {head}
        <p className="text-muted" role="status">
          Loading databases…
        </p>
      </div>
    );
  if (dbs.state === 'error')
    return (
      <div className="lc-page flex flex-col gap-3">
        {head}
        <Notice tone="danger" title="Could not list databases">
          {dbs.message}
        </Notice>
        <div>
          <Button onClick={() => setTick((t) => t + 1)}>Try again</Button>
        </div>
      </div>
    );
  if (dbs.value.length === 0)
    return (
      <div className="lc-page">
        {head}
        <Empty title="No SQL databases yet" seed={47}>
          Databases created in namespace {namespace} appear here.
        </Empty>
      </div>
    );

  const db = dbs.value.find((d) => d.id === selected);
  return (
    <div className="lc-page flex flex-col gap-4">
      {head}
      {mocked && (
        <SampleDataNotice>
          The loams.sqldb.v1 API has not landed yet; nothing here reaches a database.
        </SampleDataNotice>
      )}
      <div className="grid gap-4 md:grid-cols-[16rem_1fr]">
        <DatabaseList databases={dbs.value} selected={selected} onSelect={setSelected} />
        {db && (
          <DatabaseDetail
            key={db.id}
            client={client}
            namespace={namespace}
            db={db}
            onChange={replace}
            copy={copy}
          />
        )}
      </div>
    </div>
  );
}

function DatabaseList({
  databases,
  selected,
  onSelect,
}: {
  databases: Database[];
  selected: string;
  onSelect: (id: string) => void;
}) {
  return (
    <nav aria-label="Databases">
      <ul className="m-0 flex list-none flex-col gap-1 p-0">
        {databases.map((d) => (
          <li key={d.id}>
            <button
              type="button"
              className="flex w-full cursor-pointer items-center justify-between gap-2 rounded-sm border-0 bg-transparent px-2 py-1.5 text-left text-ink hover:bg-raised aria-[current=true]:bg-accent-soft"
              aria-current={d.id === selected ? 'true' : undefined}
              onClick={() => onSelect(d.id)}
            >
              <span className="font-medium">{d.name}</span>
              <Stage stage={d.stage} />
            </button>
          </li>
        ))}
      </ul>
    </nav>
  );
}

function DatabaseDetail({
  client,
  namespace,
  db,
  onChange,
  copy,
}: {
  client: SqlClient;
  namespace: string;
  db: Database;
  onChange: (db: Database) => void;
  copy: (text: string) => void | Promise<void>;
}) {
  const [branches, setBranches] = useState<Load<Branch[]>>({ state: 'loading' });
  const [branchId, setBranchId] = useState(db.defaultBranchId);
  const [actionError, setActionError] = useState('');
  const [acting, setActing] = useState(false);

  useEffect(() => {
    let live = true;
    client.listBranches(namespace, db.id).then(
      (value) => live && setBranches({ state: 'ready', value }),
      (e) => live && setBranches({ state: 'error', message: failureMessage(e) }),
    );
    return () => {
      live = false;
    };
  }, [client, namespace, db.id]);

  // Read the database back until a transitional stage settles.
  useEffect(() => {
    if (!isTransitional(db.stage)) return;
    let live = true;
    const t = setTimeout(() => {
      client.getDatabase(namespace, db.id).then(
        (d) => live && onChange(d),
        (e) => live && setActionError(failureMessage(e)),
      );
    }, POLL_MS);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [client, namespace, db, onChange]);

  const act = async (kind: 'suspend' | 'resume') => {
    setActing(true);
    setActionError('');
    try {
      const key = idempotencyKey();
      const next =
        kind === 'suspend'
          ? await client.suspendDatabase(namespace, db.id, key)
          : await client.resumeDatabase(namespace, db.id, key);
      onChange(next);
    } catch (e) {
      setActionError(failureMessage(e));
    } finally {
      setActing(false);
    }
  };

  return (
    <section aria-labelledby="sql-db-title" className="flex flex-col gap-4">
      <header className="flex flex-wrap items-center gap-3">
        <h2 id="sql-db-title" className="text-lg font-semibold">
          {db.name}
        </h2>
        <span role="status" aria-label={`State of ${db.name}`}>
          <Stage stage={db.stage} />
        </span>
        <span className="text-faint text-xs tabular-nums">
          {db.engineVersion} · class {db.computeClass} · {formatBytes(db.storageBytes)}
        </span>
        <div className="ml-auto flex gap-2">
          {canSuspend(db.stage) && (
            <Button disabled={acting} onClick={() => void act('suspend')}>
              Suspend
            </Button>
          )}
          {canResume(db.stage) && (
            <Button variant="primary" disabled={acting} onClick={() => void act('resume')}>
              Resume
            </Button>
          )}
        </div>
      </header>
      <p className="text-muted text-sm">
        {db.suspendAfterSeconds > 0
          ? `Suspends after ${Math.round(db.suspendAfterSeconds / 60)} min idle. A new connection wakes it.`
          : 'Never suspends when idle.'}
      </p>
      {actionError && (
        <Notice tone="danger" title="The action failed">
          {actionError}
        </Notice>
      )}

      <section aria-labelledby="sql-branches-title" className="flex flex-col gap-2">
        <h3 id="sql-branches-title" className="text-sm font-semibold">
          Branches
        </h3>
        {branches.state === 'loading' && (
          <p className="text-muted" role="status">
            Loading branches…
          </p>
        )}
        {branches.state === 'error' && (
          <Notice tone="danger" title="Could not list branches">
            {branches.message}
          </Notice>
        )}
        {branches.state === 'ready' && (
          <BranchTable branches={branches.value} selected={branchId} onSelect={setBranchId} />
        )}
      </section>

      <Connection
        client={client}
        namespace={namespace}
        databaseId={db.id}
        branchId={branchId}
        copy={copy}
      />
      <Query
        client={client}
        namespace={namespace}
        databaseId={db.id}
        branchId={branchId}
        running={db.stage === 'running'}
      />
    </section>
  );
}

function BranchTable({
  branches,
  selected,
  onSelect,
}: {
  branches: Branch[];
  selected: string;
  onSelect: (id: string) => void;
}) {
  const nameOf = (id: string) => branches.find((b) => b.id === id)?.name ?? id;
  return (
    <div className="loams-table-wrap">
      <table className="loams-table">
        <caption className="sr-only">Branches</caption>
        <thead>
          <tr>
            <th scope="col">Branch</th>
            <th scope="col">State</th>
            <th scope="col">Parent</th>
            <th scope="col">Copied at (TSO)</th>
            <th scope="col">
              <span className="sr-only">Select</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {branches.map((b) => (
            <tr key={b.id} aria-selected={b.id === selected}>
              <td>
                <span className="font-medium">{b.name}</span>{' '}
                <span className={/* mono-ok: id */ 'text-faint text-xs font-mono'}>{b.id}</span>
              </td>
              <td>
                <Stage stage={b.stage} />
              </td>
              <td>{b.parentId ? nameOf(b.parentId) : '—'}</td>
              <td className={/* mono-ok: id */ 'font-mono'}>{b.pointTs || '—'}</td>
              <td>
                <Button
                  size="sm"
                  variant={b.id === selected ? 'primary' : 'secondary'}
                  aria-pressed={b.id === selected}
                  aria-label={`Use branch ${b.name}`}
                  onClick={() => onSelect(b.id)}
                >
                  {b.id === selected ? 'In use' : 'Use'}
                </Button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
