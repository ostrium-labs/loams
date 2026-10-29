// Spark sessions (Spark Connect) and batch jobs on one Spark instance.

import {
  Badge,
  Button,
  Card,
  Field,
  formatRelative,
  Input,
  Notice,
  Select,
  SkeletonList,
  Textarea,
} from '@loams/ui';
import { type FormEvent, useState } from 'react';
import {
  type EnginesClient,
  errorText,
  type Instance,
  type SparkJobKind,
  type SparkJobState,
  type SparkSessionKind,
  type SparkSessionState,
} from '../client.js';
import { EnginesHead, instanceHref, LoadError, useLoad } from '../shared.js';

const JOB_VARIANT: Record<SparkJobState, 'neutral' | 'info' | 'success' | 'danger'> = {
  queued: 'neutral',
  running: 'info',
  succeeded: 'success',
  failed: 'danger',
  cancelled: 'neutral',
};

const SESSION_VARIANT: Record<SparkSessionState, 'neutral' | 'info' | 'success' | 'danger'> = {
  starting: 'info',
  idle: 'success',
  busy: 'info',
  closed: 'neutral',
  failed: 'danger',
};

export function SparkPage({
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
        <EnginesHead client={client} title="Sessions and jobs" crumbs={crumbs()} />
        {inst.state === 'loading' ? (
          <SkeletonList label="Loading the instance" rows={3} />
        ) : (
          <LoadError message={inst.message} />
        )}
      </div>
    );
  }
  const i = inst.value;
  return (
    <div className="lc-page">
      <EnginesHead
        client={client}
        title={`Sessions and jobs on ${i.name}`}
        crumbs={crumbs(i)}
        subtitle="Interactive Spark Connect sessions, and batch jobs that read and write Iceberg tables."
      />
      {i.kind !== 'spark' ? (
        <Notice title="Not a Spark instance">
          Sessions and jobs are Spark's; use the SQL workspace for {i.name}.
        </Notice>
      ) : (
        <>
          <SessionsCard client={client} namespace={namespace} id={id} />
          <JobsCard client={client} namespace={namespace} id={id} />
        </>
      )}
    </div>
  );
}

function SessionsCard({
  client,
  namespace,
  id,
}: {
  client: EnginesClient;
  namespace: string;
  id: string;
}) {
  const [sessions, reload] = useLoad(
    () => client.listSessions(namespace, id),
    [client, namespace, id],
  );
  const [kind, setKind] = useState<SparkSessionKind>('sql');
  const [failure, setFailure] = useState<string | undefined>();
  const guard = async (f: () => Promise<unknown>) => {
    setFailure(undefined);
    try {
      await f();
      reload();
    } catch (e) {
      setFailure(errorText(e));
    }
  };
  return (
    <Card
      title="Sessions"
      actions={
        <div className="flex items-center gap-2">
          <Select
            aria-label="Session kind"
            value={kind}
            onChange={(e) => setKind(e.currentTarget.value as SparkSessionKind)}
          >
            <option value="sql">SQL</option>
            <option value="python">Python</option>
          </Select>
          <Button
            variant="primary"
            onClick={() => guard(() => client.createSession(namespace, id, { kind }))}
          >
            Start session
          </Button>
        </div>
      }
    >
      {failure && (
        <Notice tone="danger" title="Session action failed">
          {failure}
        </Notice>
      )}
      {sessions.state === 'loading' && <SkeletonList label="Loading sessions" rows={2} />}
      {sessions.state === 'error' && <LoadError message={sessions.message} />}
      {sessions.state === 'ok' &&
        (sessions.value.length === 0 ? (
          <p className="text-muted">No sessions.</p>
        ) : (
          <div className="loams-table-wrap">
            <table className="loams-table" aria-label="Spark sessions">
              <thead>
                <tr>
                  <th>Session</th>
                  <th>Kind</th>
                  <th>State</th>
                  <th>Connect</th>
                  <th>Started</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {sessions.value.map((s) => (
                  <tr key={s.id}>
                    <td className={/* mono-ok: id */ 'font-mono'}>{s.id}</td>
                    <td>{s.kind}</td>
                    <td>
                      <Badge variant={SESSION_VARIANT[s.state]}>{s.state}</Badge>
                    </td>
                    <td className={/* mono-ok: path */ 'font-mono'}>{s.connect_url ?? '—'}</td>
                    <td>{formatRelative(s.created_at)}</td>
                    <td>
                      {s.state !== 'closed' && s.state !== 'failed' && (
                        <Button
                          size="xs"
                          variant="quiet"
                          aria-label={`Close session ${s.id}`}
                          onClick={() => guard(() => client.closeSession(namespace, id, s.id))}
                        >
                          Close
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ))}
    </Card>
  );
}

function JobsCard({
  client,
  namespace,
  id,
}: {
  client: EnginesClient;
  namespace: string;
  id: string;
}) {
  const [jobs, reload] = useLoad(() => client.listJobs(namespace, id), [client, namespace, id]);
  const [name, setName] = useState('');
  const [kind, setKind] = useState<SparkJobKind>('sql');
  const [entrypoint, setEntrypoint] = useState('');
  const [args, setArgs] = useState('');
  const [failure, setFailure] = useState<string | undefined>();
  const [busy, setBusy] = useState(false);

  const guard = async (f: () => Promise<unknown>) => {
    setFailure(undefined);
    setBusy(true);
    try {
      await f();
      reload();
      return true;
    } catch (e) {
      setFailure(errorText(e));
      return false;
    } finally {
      setBusy(false);
    }
  };
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim() || !entrypoint.trim()) return;
    const argv = args.trim() ? args.trim().split(/\s+/) : [];
    const ok = await guard(() =>
      client.submitJob(namespace, id, {
        name: name.trim(),
        kind,
        entrypoint: entrypoint.trim(),
        args: argv,
      }),
    );
    if (ok) {
      setName('');
      setEntrypoint('');
      setArgs('');
    }
  }

  return (
    <Card title="Batch jobs">
      <form className="flex flex-col gap-3" onSubmit={submit} aria-label="Submit a Spark job">
        <div className="flex flex-wrap gap-3">
          <Field label="Job name">
            {(p) => <Input {...p} value={name} onChange={(e) => setName(e.currentTarget.value)} />}
          </Field>
          <Field label="Kind">
            {(p) => (
              <Select
                {...p}
                value={kind}
                onChange={(e) => setKind(e.currentTarget.value as SparkJobKind)}
              >
                <option value="sql">SQL</option>
                <option value="python">Python</option>
                <option value="jar">JAR</option>
              </Select>
            )}
          </Field>
        </div>
        <Field
          label={kind === 'sql' ? 'SQL' : 'Entrypoint'}
          hint={kind === 'sql' ? undefined : 'An object-storage path: s3://bucket/job.py or .jar.'}
        >
          {(p) => (
            <Textarea
              {...p}
              rows={kind === 'sql' ? 4 : 1}
              value={entrypoint}
              onChange={(e) => setEntrypoint(e.currentTarget.value)}
            />
          )}
        </Field>
        {kind !== 'sql' && (
          <Field label="Arguments" hint="Separated by spaces.">
            {(p) => <Input {...p} value={args} onChange={(e) => setArgs(e.currentTarget.value)} />}
          </Field>
        )}
        <div>
          <Button
            type="submit"
            variant="primary"
            disabled={busy || !name.trim() || !entrypoint.trim()}
          >
            Submit job
          </Button>
        </div>
      </form>
      {failure && (
        <Notice tone="danger" title="Job action failed">
          {failure}
        </Notice>
      )}
      {jobs.state === 'loading' && <SkeletonList label="Loading jobs" rows={2} />}
      {jobs.state === 'error' && <LoadError message={jobs.message} />}
      {jobs.state === 'ok' &&
        (jobs.value.length === 0 ? (
          <p className="text-muted">No jobs yet.</p>
        ) : (
          <div className="loams-table-wrap">
            <table className="loams-table" aria-label="Spark jobs">
              <thead>
                <tr>
                  <th>Job</th>
                  <th>Kind</th>
                  <th>State</th>
                  <th>Entrypoint</th>
                  <th>Submitted</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {jobs.value.map((j) => (
                  <tr key={j.id}>
                    <td>
                      <div>{j.name}</div>
                      <div className={/* mono-ok: id */ 'font-mono text-muted text-small'}>
                        {j.id}
                      </div>
                    </td>
                    <td>{j.kind}</td>
                    <td>
                      <Badge variant={JOB_VARIANT[j.state]}>{j.state}</Badge>
                      {j.message && <div className="text-muted text-small">{j.message}</div>}
                    </td>
                    <td className={/* mono-ok: path */ 'font-mono'}>
                      {j.entrypoint}
                      {j.args.length > 0 && ` ${j.args.join(' ')}`}
                    </td>
                    <td>{formatRelative(j.submitted_at)}</td>
                    <td>
                      {(j.state === 'queued' || j.state === 'running') && (
                        <Button
                          size="xs"
                          variant="quiet"
                          aria-label={`Cancel job ${j.name}`}
                          onClick={() => guard(() => client.cancelJob(namespace, id, j.id))}
                        >
                          Cancel
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ))}
    </Card>
  );
}
