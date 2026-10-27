// The schema sidebar (§48 §18.2): GetSchema of the selected graph, its labels and edge
// types with their counts, its property keys and its indexes.

import type { graph } from '@loams/proto';
import { Button, SkeletonList } from '@loams/ui';
import { type ReactNode, useEffect, useState } from 'react';
import { type GraphClient, type GraphFailure, toFailure } from './client.js';

const fmt = new Intl.NumberFormat('en');

export function Schema({
  client,
  namespace,
  name,
  refresh,
  onSchema,
}: {
  client: GraphClient;
  namespace: string;
  name: string;
  /** Changes after each statement that wrote, so counts follow. */
  refresh: number;
  /** Gets the schema each time it is read (the editor completes its names). */
  onSchema?: (schema: graph.GraphSchema | undefined) => void;
}) {
  const [schema, setSchema] = useState<graph.GraphSchema>();
  const [error, setError] = useState<GraphFailure>();
  const [tick, setTick] = useState(0);

  // oxlint-disable-next-line react/exhaustive-deps -- refresh and tick re-read on purpose
  useEffect(() => {
    let live = true;
    setError(undefined);
    client
      .getSchema(namespace, name)
      .then((s) => {
        if (!live) return;
        setSchema(s);
        onSchema?.(s);
      })
      .catch((e) => {
        if (!live) return;
        setSchema(undefined);
        setError(toFailure(e));
      });
    return () => {
      live = false;
    };
  }, [client, namespace, name, refresh, tick]);

  return (
    <aside className="flex flex-col gap-3 text-body" aria-label="Schema">
      <div className="flex items-center justify-between gap-2">
        <h2 className="m-0 text-emphasis">Schema</h2>
        <Button size="sm" variant="quiet" onClick={() => setTick((t) => t + 1)}>
          Refresh
        </Button>
      </div>
      {error && <p className="m-0 text-danger">{error.message}</p>}
      {!schema && !error && <SkeletonList label="Loading the schema" rows={4} />}
      {schema && (
        <>
          <Group title="Labels" empty="No labels.">
            {schema.labels.map((l) => (
              <Row key={l.label} name={`:${l.label}`} count={l.count} />
            ))}
          </Group>
          <Group title="Relationship types" empty="No relationship types.">
            {schema.edgeTypes.map((t) => (
              <Row key={t.type} name={`:${t.type}`} count={t.count} />
            ))}
          </Group>
          <Group title="Property keys" empty="No property keys.">
            {schema.propertyKeys.map((k) => (
              <li key={k} className={/* mono-ok: id */ 'font-mono'}>
                {k}
              </li>
            ))}
          </Group>
          <Group title="Indexes" empty="No indexes.">
            {schema.indexes.map((i) => (
              <li key={i.name} className="flex flex-col">
                <span className={/* mono-ok: id */ 'font-mono'}>{i.name}</span>
                <span className="text-small text-muted">
                  {[i.kind, i.target, i.properties.join(', ')].filter(Boolean).join(' · ')}
                </span>
              </li>
            ))}
          </Group>
        </>
      )}
    </aside>
  );
}

function Group({
  title,
  empty,
  children,
}: {
  title: string;
  empty: string;
  children: ReactNode[];
}) {
  return (
    <section className="flex flex-col gap-1">
      <h3 className="m-0 text-small font-medium text-muted">{title}</h3>
      {children.length ? (
        <ul className="m-0 flex list-none flex-col gap-1 p-0" aria-label={title}>
          {children}
        </ul>
      ) : (
        <p className="m-0 text-faint">{empty}</p>
      )}
    </section>
  );
}

function Row({ name, count }: { name: string; count: bigint }) {
  return (
    <li className="flex items-center justify-between gap-2">
      <span className={/* mono-ok: id */ 'font-mono'}>{name}</span>
      <span className="text-muted tabular-nums">{fmt.format(count)}</span>
    </li>
  );
}
