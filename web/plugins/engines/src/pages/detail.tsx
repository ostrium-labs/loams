// One engine instance: phase, health checks, endpoints, size and version, lifecycle actions
// and the metering samples it emitted (vCPU-seconds, memory GB-seconds, state bytes).

import {
  Button,
  Card,
  ConfirmDialog,
  Dialog,
  Field,
  formatBytes,
  formatRelative,
  Notice,
  Select,
  SkeletonList,
  Stat,
  Stats,
} from '@loams/ui';
import { useEffect, useState } from 'react';
import {
  type EnginesClient,
  gibHours,
  type Instance,
  isTransitional,
  KIND_LABELS,
  type SizeTier,
  sizeText,
  type Usage,
  vcpuHours,
} from '../client.js';
import {
  ActionError,
  EnginesHead,
  HealthBadge,
  instanceHref,
  LoadError,
  PhaseTag,
  useLoad,
} from '../shared.js';

/** How often a changing instance is read again. */
export const POLL_MS = 3000;

type Pending = 'suspend' | 'resume' | 'delete' | 'scale' | 'upgrade' | undefined;

export function DetailPage({
  client,
  navigate,
  namespace,
  id,
  copy,
  pollMs = POLL_MS,
}: {
  client: EnginesClient;
  navigate: (to: string) => void;
  namespace: string;
  id: string;
  copy?: (text: string) => void | Promise<void>;
  pollMs?: number;
}) {
  const [inst, reload] = useLoad(() => client.get(namespace, id), [client, namespace, id]);
  const [usage] = useLoad(() => client.usage(namespace, id), [client, namespace, id]);
  const [dialog, setDialog] = useState<Pending>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<unknown>();

  const phase = inst.state === 'ok' ? inst.value.status.phase : undefined;
  useEffect(() => {
    if (!phase || !isTransitional(phase) || pollMs <= 0) return;
    const t = setInterval(reload, pollMs);
    return () => clearInterval(t);
  }, [phase, pollMs, reload]);

  const crumbs = (
    <a
      href="/engines"
      onClick={(e) => {
        e.preventDefault();
        navigate('/engines');
      }}
    >
      Engines
    </a>
  );
  if (inst.state !== 'ok') {
    return (
      <div className="lc-page">
        <EnginesHead client={client} title={id} crumbs={crumbs} />
        {inst.state === 'loading' ? (
          <SkeletonList label="Loading the instance" layout="cards" rows={3} />
        ) : (
          <LoadError message={inst.message} />
        )}
      </div>
    );
  }
  const i = inst.value;

  async function act(what: Pending, run: () => Promise<Instance>) {
    setBusy(true);
    setFailure(undefined);
    try {
      await run();
      setDialog(undefined);
      if (what === 'delete') navigate('/engines');
      else reload();
    } catch (e) {
      setFailure(e);
      setDialog(undefined);
    } finally {
      setBusy(false);
    }
  }

  const changing = isTransitional(i.status.phase);
  const canSql = i.kind !== 'spark';
  return (
    <div className="lc-page">
      <EnginesHead
        client={client}
        title={i.name}
        crumbs={crumbs}
        subtitle={`${KIND_LABELS[i.kind]} in namespace ${i.namespace}`}
        actions={
          <>
            {canSql && (
              <Button onClick={() => navigate(instanceHref(i, '/sql'))}>SQL workspace</Button>
            )}
            {i.kind === 'spark' && (
              <Button onClick={() => navigate(instanceHref(i, '/spark'))}>Sessions and jobs</Button>
            )}
            {i.status.phase === 'suspended' ? (
              <Button onClick={() => setDialog('resume')} disabled={busy}>
                Resume
              </Button>
            ) : (
              <Button
                onClick={() => setDialog('suspend')}
                disabled={busy || changing || i.status.phase === 'failed'}
              >
                Suspend
              </Button>
            )}
            <Button onClick={() => setDialog('scale')} disabled={busy || changing}>
              Scale
            </Button>
            <Button onClick={() => setDialog('upgrade')} disabled={busy || changing}>
              Upgrade
            </Button>
            <Button variant="danger" onClick={() => setDialog('delete')} disabled={busy}>
              Delete
            </Button>
          </>
        }
      />
      {failure !== undefined && <ActionError error={failure} title="The action failed" />}
      {i.status.message && (
        <Notice tone={i.status.phase === 'failed' ? 'danger' : 'info'} title="Status">
          {i.status.message}
        </Notice>
      )}
      <Stats>
        <Stat label="Phase" value={<PhaseTag phase={i.status.phase} />} />
        <Stat label="Health" value={<HealthBadge health={i.status.health} />} />
        <Stat
          label="Version"
          value={
            <span className={/* mono-ok: id */ 'font-mono'}>
              {i.status.version ?? i.version ?? '—'}
            </span>
          }
          detail={
            i.version && i.status.version && i.version !== i.status.version
              ? `upgrading to ${i.version}`
              : undefined
          }
        />
        <Stat label="Size" value={sizeText(i.size)} />
        <Stat label="Observed" value={formatRelative(i.status.observed_at)} />
      </Stats>

      <Card title="Endpoints">
        {i.endpoints.length === 0 ? (
          <p className="text-muted">
            No endpoints yet: they appear once the instance is running. Every endpoint is a
            loams-gateway address; the engine itself is never exposed.
          </p>
        ) : (
          <ul className="flex flex-col gap-2" aria-label="Endpoints">
            {i.endpoints.map((e) => (
              <li key={e.name} className="flex flex-wrap items-center gap-2">
                <strong>{e.name}</strong>
                <span className="text-muted text-small">{e.protocol}</span>
                {e.internal && <span className="text-muted text-small">(console only)</span>}
                <code className="font-mono">{e.url}</code>
                {copy && (
                  <Button size="xs" variant="quiet" onClick={() => copy(e.url)}>
                    Copy
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card title="Health checks">
        {i.status.checks.length === 0 ? (
          <p className="text-muted">No checks reported.</p>
        ) : (
          <ul className="flex flex-col gap-1" aria-label="Health checks">
            {i.status.checks.map((c) => (
              <li key={c.name} className="flex items-center gap-2">
                <HealthBadge health={c.state} />
                <span>{c.name}</span>
                {c.message && <span className="text-muted">{c.message}</span>}
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card title="Usage">
        {usage.state === 'loading' && <SkeletonList label="Loading usage" rows={3} />}
        {usage.state === 'error' && <LoadError message={usage.message} />}
        {usage.state === 'ok' && <UsageView usage={usage.value} />}
      </Card>

      <Card title="Storage">
        <p className="text-muted">
          State and checkpoints live under <code className="font-mono">{i.state_prefix}</code> in
          Loams object storage.
        </p>
      </Card>

      <ConfirmDialog
        open={dialog === 'suspend'}
        onClose={() => setDialog(undefined)}
        onConfirm={() => act('suspend', () => client.suspend(namespace, id))}
        object={i.name}
        impact="Compute stops and is no longer billed; state is kept and resume starts from it."
        confirm="Suspend instance"
        pending={busy}
      />
      <ConfirmDialog
        open={dialog === 'resume'}
        onClose={() => setDialog(undefined)}
        onConfirm={() => act('resume', () => client.resume(namespace, id))}
        object={i.name}
        impact="Compute starts again from the kept state."
        confirm="Resume instance"
        pending={busy}
      />
      <ConfirmDialog
        open={dialog === 'delete'}
        onClose={() => setDialog(undefined)}
        onConfirm={() => act('delete', () => client.remove(namespace, id))}
        object={i.name}
        impact="The instance, its endpoints and its state in object storage are deleted. This cannot be undone."
        confirm="Delete instance"
        pending={busy}
      />
      <ScaleDialog
        open={dialog === 'scale'}
        instance={i}
        busy={busy}
        onClose={() => setDialog(undefined)}
        onScale={(tier, replicas) =>
          act('scale', () => client.update(namespace, id, { size: { tier, replicas } }))
        }
      />
      <UpgradeDialog
        open={dialog === 'upgrade'}
        client={client}
        instance={i}
        busy={busy}
        onClose={() => setDialog(undefined)}
        onUpgrade={(version) => act('upgrade', () => client.update(namespace, id, { version }))}
      />
    </div>
  );
}

export function UsageView({ usage }: { usage: Usage }) {
  const fmt = (n: number, digits = 1) =>
    n.toLocaleString('en-US', { maximumFractionDigits: digits });
  const estimated = usage.samples.filter((s) => s.estimated).length;
  return (
    <>
      <Stats>
        <Stat
          label="vCPU-hours"
          value={fmt(vcpuHours(usage.totals.cpu_ms), 3)}
          detail={`${fmt(usage.totals.cpu_ms / 1000, 0)} CPU-seconds`}
        />
        <Stat label="Memory GiB-hours" value={fmt(gibHours(usage.totals.memory_mib_ms), 3)} />
        <Stat label="State" value={formatBytes(usage.totals.state_bytes)} />
      </Stats>
      {estimated > 0 && (
        <p className="text-muted text-small">
          {estimated === 1 ? '1 window is' : `${estimated} windows are`} estimated from the reserved
          size, not measured.
        </p>
      )}
      {usage.samples.length > 0 && (
        <div className="loams-table-wrap">
          <table className="loams-table" aria-label="Usage samples">
            <thead>
              <tr>
                <th>Window end</th>
                <th>CPU-seconds</th>
                <th>Memory GiB-s</th>
                <th>State</th>
                <th>Source</th>
              </tr>
            </thead>
            <tbody>
              {usage.samples
                .slice(-12)
                .reverse()
                .map((s) => (
                  <tr key={s.window_end}>
                    <td>{formatRelative(s.window_end)}</td>
                    <td>{fmt(s.cpu_ms / 1000)}</td>
                    <td>{fmt(s.memory_mib_ms / 1024 / 1000)}</td>
                    <td>{formatBytes(s.state_bytes)}</td>
                    <td>{s.estimated ? 'estimated' : 'measured'}</td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function ScaleDialog({
  open,
  instance,
  busy,
  onClose,
  onScale,
}: {
  open: boolean;
  instance: Instance;
  busy: boolean;
  onClose: () => void;
  onScale: (tier: SizeTier, replicas: number) => void;
}) {
  const [tier, setTier] = useState<SizeTier>(instance.size.tier);
  const [replicas, setReplicas] = useState(String(instance.size.replicas));
  const n = Number.parseInt(replicas, 10);
  const ok = Number.isInteger(n) && n >= 1 && n <= 64;
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={`Scale ${instance.name}`}
      footer={
        <Button variant="primary" disabled={!ok || busy} onClick={() => onScale(tier, n)}>
          Scale
        </Button>
      }
    >
      <div className="flex flex-col gap-3">
        <Field label="Size">
          {(p) => (
            <Select
              {...p}
              value={tier}
              onChange={(e) => setTier(e.currentTarget.value as SizeTier)}
            >
              {(['xs', 's', 'm', 'l', 'xl'] as const).map((t) => (
                <option key={t} value={t}>
                  {t.toUpperCase()}
                </option>
              ))}
            </Select>
          )}
        </Field>
        <Field label="Replicas" error={ok ? undefined : 'From 1 to 64.'}>
          {(p) => (
            <input
              {...p}
              className="loams-input"
              type="number"
              min={1}
              max={64}
              value={replicas}
              onChange={(e) => setReplicas(e.currentTarget.value)}
            />
          )}
        </Field>
      </div>
    </Dialog>
  );
}

function UpgradeDialog({
  open,
  client,
  instance,
  busy,
  onClose,
  onUpgrade,
}: {
  open: boolean;
  client: EnginesClient;
  instance: Instance;
  busy: boolean;
  onClose: () => void;
  onUpgrade: (version: string) => void;
}) {
  const [catalog] = useLoad(() => client.catalog(), [client]);
  const versions =
    catalog.state === 'ok'
      ? (catalog.value.engines.find((e) => e.kind === instance.kind)?.versions ?? [])
      : [];
  const current = instance.status.version ?? instance.version ?? '';
  const [version, setVersion] = useState('');
  const chosen = version || versions.find((v) => v !== current) || '';
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={`Upgrade ${instance.name}`}
      footer={
        <Button
          variant="primary"
          disabled={!chosen || chosen === current || busy}
          onClick={() => onUpgrade(chosen)}
        >
          Upgrade
        </Button>
      }
    >
      <Field label="Version" hint={current ? `Running ${current}.` : undefined}>
        {(p) => (
          <Select {...p} value={chosen} onChange={(e) => setVersion(e.currentTarget.value)}>
            {versions.map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </Select>
        )}
      </Field>
    </Dialog>
  );
}
