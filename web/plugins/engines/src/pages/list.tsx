// Engines: every RisingWave, Flink SQL and Spark instance in a namespace, filtered by kind.

import { Button, Empty, formatRelative, Select, SkeletonList } from '@loams/ui';
import { useId, useState } from 'react';
import {
  ENGINE_KINDS,
  type EngineKind,
  type EnginesClient,
  KIND_LABELS,
  sizeText,
} from '../client.js';
import {
  EnginesHead,
  HealthBadge,
  instanceHref,
  KindLabel,
  LoadError,
  PhaseTag,
  useLoad,
} from '../shared.js';

export function ListPage({
  client,
  navigate,
  initialNamespace,
}: {
  client: EnginesClient;
  navigate: (to: string) => void;
  initialNamespace?: string;
}) {
  const ids = useId();
  const [namespaces] = useLoad(() => client.listNamespaces(), [client]);
  const [chosen, setChosen] = useState<string | undefined>(initialNamespace);
  const namespace =
    chosen ?? (namespaces.state === 'ok' ? (namespaces.value[0] ?? 'default') : undefined);
  const [kind, setKind] = useState<EngineKind | ''>('');
  const [list] = useLoad(
    () => (namespace ? client.list(namespace, kind || undefined) : new Promise<never>(() => {})),
    [client, namespace, kind],
  );

  return (
    <div className="lc-page">
      <EnginesHead
        client={client}
        title="Engines"
        subtitle="Managed RisingWave, Flink SQL and Spark, with state in Loams object storage."
        actions={
          <>
            <label className="flex items-center gap-2 text-body" htmlFor={`${ids}-namespace`}>
              <span className="text-muted">Namespace</span>
              <Select
                id={`${ids}-namespace`}
                aria-label="Namespace"
                value={namespace ?? ''}
                onChange={(e) => setChosen(e.currentTarget.value)}
              >
                {(namespaces.state === 'ok' ? namespaces.value : [namespace ?? 'default']).map(
                  (p) => (
                    <option key={p} value={p}>
                      {p}
                    </option>
                  ),
                )}
              </Select>
            </label>
            <label className="flex items-center gap-2 text-body" htmlFor={`${ids}-kind`}>
              <span className="text-muted">Kind</span>
              <Select
                id={`${ids}-kind`}
                aria-label="Kind"
                value={kind}
                onChange={(e) => setKind(e.currentTarget.value as EngineKind | '')}
              >
                <option value="">All kinds</option>
                {ENGINE_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {KIND_LABELS[k]}
                  </option>
                ))}
              </Select>
            </label>
            <Button variant="primary" onClick={() => navigate('/engines/new')}>
              New instance
            </Button>
          </>
        }
      />
      {list.state === 'loading' && <SkeletonList label="Loading engine instances" rows={4} />}
      {list.state === 'error' && <LoadError message={list.message} />}
      {list.state === 'ok' && list.value.length === 0 && (
        <Empty
          title="No engine instances"
          actions={
            <Button variant="primary" onClick={() => navigate('/engines/new')}>
              New instance
            </Button>
          }
        >
          Create a RisingWave or Flink SQL instance for streaming SQL, or Spark for batch jobs.
        </Empty>
      )}
      {list.state === 'ok' && list.value.length > 0 && (
        <div className="loams-table-wrap">
          <table className="loams-table" aria-label="Engine instances">
            <thead>
              <tr>
                <th>Name</th>
                <th>Kind</th>
                <th>Phase</th>
                <th>Health</th>
                <th>Version</th>
                <th>Size</th>
                <th>Updated</th>
              </tr>
            </thead>
            <tbody>
              {list.value.map((i) => (
                <tr key={i.id}>
                  <td>
                    <a
                      href={instanceHref(i)}
                      onClick={(e) => {
                        e.preventDefault();
                        navigate(instanceHref(i));
                      }}
                    >
                      {i.name}
                    </a>
                  </td>
                  <td>
                    <KindLabel kind={i.kind} />
                  </td>
                  <td>
                    <PhaseTag phase={i.status.phase} />
                  </td>
                  <td>
                    <HealthBadge health={i.status.health} />
                  </td>
                  <td className={/* mono-ok: id */ 'font-mono'}>
                    {i.status.version ?? i.version ?? '—'}
                  </td>
                  <td>{sizeText(i.size)}</td>
                  <td>{formatRelative(i.updated_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
