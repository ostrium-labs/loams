// New engine instance: kind, name, version (from the catalog), size tier and replicas.

import { Button, Card, Field, Input, Select, SkeletonList } from '@loams/ui';
import { type FormEvent, useState } from 'react';
import {
  type EngineKind,
  type EnginesClient,
  type InstanceCreate,
  KIND_LABELS,
  type SizeTier,
} from '../client.js';
import { ActionError, EnginesHead, instanceHref, LoadError, useLoad } from '../shared.js';

const NAME = /^[a-z]([a-z0-9-]{0,38}[a-z0-9])?$/;

export function CreatePage({
  client,
  navigate,
}: {
  client: EnginesClient;
  navigate: (to: string) => void;
}) {
  const [catalog] = useLoad(() => client.catalog(), [client]);
  const [namespaces] = useLoad(() => client.listNamespaces(), [client]);
  const [namespace, setNamespace] = useState<string | undefined>();
  const [kind, setKind] = useState<EngineKind>('risingwave');
  const [name, setName] = useState('');
  const [version, setVersion] = useState('');
  const [tier, setTier] = useState<SizeTier>('s');
  const [replicas, setReplicas] = useState('1');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<unknown>();

  const head = (
    <EnginesHead
      client={client}
      title="New engine instance"
      crumbs={
        <a
          href="/engines"
          onClick={(e) => {
            e.preventDefault();
            navigate('/engines');
          }}
        >
          Engines
        </a>
      }
    />
  );
  if (catalog.state === 'loading') {
    return (
      <div className="lc-page">
        {head}
        <SkeletonList label="Loading engine kinds" rows={3} />
      </div>
    );
  }
  if (catalog.state === 'error') {
    return (
      <div className="lc-page">
        {head}
        <LoadError message={catalog.message} />
      </div>
    );
  }
  const { engines, sizes } = catalog.value;
  const info = engines.find((e) => e.kind === kind);
  const chosenNamespace =
    namespace ?? (namespaces.state === 'ok' ? (namespaces.value[0] ?? 'default') : 'default');
  const replicaCount = Number.parseInt(replicas, 10);
  const nameError = name && !NAME.test(name) ? 'Lowercase letters, digits and -, 1 to 40.' : '';
  const replicaError =
    !Number.isInteger(replicaCount) || replicaCount < 1 || replicaCount > 64 ? 'From 1 to 64.' : '';

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name || nameError || replicaError) return;
    const req: InstanceCreate = {
      name,
      kind,
      size: { tier, replicas: replicaCount },
      ...(version ? { version } : {}),
    };
    setBusy(true);
    setFailure(undefined);
    try {
      const created = await client.create(chosenNamespace, req);
      navigate(instanceHref(created));
    } catch (err) {
      setFailure(err);
      setBusy(false);
    }
  }

  return (
    <div className="lc-page">
      {head}
      <Card>
        <form className="flex max-w-xl flex-col gap-4" onSubmit={submit} aria-label="New engine">
          <Field label="Namespace">
            {(p) => (
              <Select
                {...p}
                value={chosenNamespace}
                onChange={(e) => setNamespace(e.currentTarget.value)}
              >
                {(namespaces.state === 'ok' ? namespaces.value : [chosenNamespace]).map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Field
            label="Engine"
            hint={
              info && !info.enabled
                ? 'This server does not run this engine; turn its profile on first.'
                : 'Every engine is a preview.'
            }
          >
            {(p) => (
              <Select
                {...p}
                value={kind}
                onChange={(e) => {
                  setKind(e.currentTarget.value as EngineKind);
                  setVersion('');
                }}
              >
                {engines.map((e) => (
                  <option key={e.kind} value={e.kind}>
                    {e.label || KIND_LABELS[e.kind]}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Field label="Name" error={nameError || undefined}>
            {(p) => (
              <Input
                {...p}
                value={name}
                placeholder="orders-stream"
                autoComplete="off"
                onChange={(e) => setName(e.currentTarget.value)}
              />
            )}
          </Field>
          <Field label="Version">
            {(p) => (
              <Select {...p} value={version} onChange={(e) => setVersion(e.currentTarget.value)}>
                <option value="">
                  {info?.default_version ? `Default (${info.default_version})` : 'Default'}
                </option>
                {(info?.versions ?? []).map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Field label="Size" hint="Per replica.">
            {(p) => (
              <Select
                {...p}
                value={tier}
                onChange={(e) => setTier(e.currentTarget.value as SizeTier)}
              >
                {sizes.map((s) => (
                  <option key={s.tier} value={s.tier}>
                    {`${s.tier.toUpperCase()}: ${s.vcpu_milli / 1000} vCPU, ${s.memory_mib / 1024} GiB`}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Field
            label={
              kind === 'flink' ? 'TaskManagers' : kind === 'spark' ? 'Executors' : 'Compute nodes'
            }
            error={replicaError || undefined}
          >
            {(p) => (
              <Input
                {...p}
                type="number"
                min={1}
                max={64}
                value={replicas}
                onChange={(e) => setReplicas(e.currentTarget.value)}
              />
            )}
          </Field>
          {failure !== undefined && (
            <ActionError error={failure} title="Could not create the instance" />
          )}
          <div className="flex gap-2">
            <Button
              type="submit"
              variant="primary"
              disabled={busy || !name || Boolean(nameError) || Boolean(replicaError)}
            >
              Create instance
            </Button>
            <Button variant="quiet" onClick={() => navigate('/engines')}>
              Cancel
            </Button>
          </div>
        </form>
      </Card>
    </div>
  );
}
