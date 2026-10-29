// The SQL workspace for RisingWave (Postgres dialect) and Flink SQL, on the shared
// CodeMirror workbench from @loams/ui. A Flink INSERT starts a streaming job; its `notice`
// says which.

import { Notice, type QueryOutcome, SkeletonList, SqlWorkbench } from '@loams/ui';
import { useCallback, useState } from 'react';
import { type EnginesClient, type Instance, KIND_LABELS, type SqlResult } from '../client.js';
import { EnginesHead, instanceHref, LoadError, PhaseTag, useLoad } from '../shared.js';

/** Rows the workspace asks for; the server caps it too. */
export const SQL_MAX_ROWS = 1000;

export function toOutcome(r: SqlResult): QueryOutcome {
  return {
    columns: r.columns.map((c) => ({ name: c.name, type: c.type })),
    rows: r.rows,
    truncated: r.truncated,
    elapsedMs: r.elapsed_ms,
    ...(r.columns.length === 0 ? { rowsAffected: r.row_count } : {}),
  };
}

const STARTERS: Record<'risingwave' | 'flink', string> = {
  risingwave:
    'CREATE MATERIALIZED VIEW revenue_by_region AS\n  SELECT region, count(*) AS orders, sum(amount) AS revenue\n  FROM orders GROUP BY region;',
  flink:
    "CREATE TABLE orders (region STRING, amount DECIMAL(18, 2))\n  WITH ('connector' = 'kafka', 'topic' = 'orders', 'format' = 'json');",
};

export function SqlPage({
  client,
  navigate,
  namespace,
  id,
}: {
  client: EnginesClient;
  navigate: (to: string) => void;
  namespace: string;
  id: string;
}) {
  const [inst] = useLoad(() => client.get(namespace, id), [client, namespace, id]);
  const [notice, setNotice] = useState<string | undefined>();
  const run = useCallback(
    async (text: string, signal: AbortSignal) => {
      const r = await client.sql(namespace, id, { sql: text, max_rows: SQL_MAX_ROWS }, signal);
      setNotice(r.notice ?? undefined);
      return toOutcome(r);
    },
    [client, namespace, id],
  );

  const crumbs = (i?: Instance) => (
    <>
      <a
        href="/engines"
        onClick={(e) => {
          e.preventDefault();
          navigate('/engines');
        }}
      >
        Engines
      </a>
      {i && (
        <>
          {' / '}
          <a
            href={instanceHref(i)}
            onClick={(e) => {
              e.preventDefault();
              navigate(instanceHref(i));
            }}
          >
            {i.name}
          </a>
        </>
      )}
    </>
  );
  if (inst.state !== 'ok') {
    return (
      <div className="lc-page">
        <EnginesHead client={client} title="SQL" crumbs={crumbs()} />
        {inst.state === 'loading' ? (
          <SkeletonList label="Loading the instance" rows={3} />
        ) : (
          <LoadError message={inst.message} />
        )}
      </div>
    );
  }
  const i = inst.value;
  if (i.kind === 'spark') {
    return (
      <div className="lc-page">
        <EnginesHead client={client} title="SQL" crumbs={crumbs(i)} />
        <Notice title="Spark runs SQL in a session">
          Open Sessions and jobs to start a Spark SQL session or submit a SQL job.
        </Notice>
      </div>
    );
  }
  const running = i.status.phase === 'running';
  return (
    <div className="lc-page">
      <EnginesHead
        client={client}
        title={`SQL on ${i.name}`}
        crumbs={crumbs(i)}
        subtitle={
          i.kind === 'risingwave'
            ? 'RisingWave speaks the Postgres dialect: sources, tables, materialized views and sinks.'
            : 'Flink SQL: an INSERT INTO starts a streaming job that keeps running.'
        }
        actions={<PhaseTag phase={i.status.phase} />}
      />
      {notice && (
        <Notice tone="success" title="Started">
          {notice}
        </Notice>
      )}
      <SqlWorkbench
        dialect={i.kind === 'risingwave' ? 'postgres' : 'generic'}
        run={run}
        historyScope={`engines:${i.namespace}/${i.id}`}
        label="SQL"
        placeholder={STARTERS[i.kind]}
        disabled={!running}
        disabledReason={
          running ? undefined : `${KIND_LABELS[i.kind]} instance ${i.name} is ${i.status.phase}.`
        }
        resetKey={i.id}
      />
    </div>
  );
}
