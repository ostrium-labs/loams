// The Postgres control plane view (PG2 Tasks 5-7): projects in a namespace, the selected
// project's branch tree, roles and databases on the selected branch, and the operations
// the control plane's reconcilers are working through. Creates and deletes return an
// operation; the view polls operations while any is open and reloads the project and its
// branches when one finishes.

import { NamespacePicker } from '@loams/desktop-ui';
import {
  Badge,
  Button,
  Dialog,
  Empty,
  Field,
  Input,
  Notice,
  SampleDataNotice,
  Select,
} from '@loams/ui';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { BranchAccess } from './BranchAccess.js';
import { BranchTree } from './BranchTree.js';
import { Operations } from './Operations.js';
import {
  failureText,
  type PgBranch,
  type PgControlClient,
  type PgOperation,
  type PgProject,
  TERMINAL_OPERATION_STATES,
} from './types.js';

export const PG_VERSIONS = [17, 16, 15, 14] as const;
const NAMESPACES_KEY = 'loams.postgres.namespaces';

export interface ControlPlaneProps {
  client: PgControlClient;
  namespace?: string;
  /** How often to poll operations while one is open. */
  pollMs?: number;
  copy?: (text: string) => Promise<void> | void;
}

const isOpen = (op: PgOperation) => !TERMINAL_OPERATION_STATES.includes(op.state);

export function ControlPlane({
  client,
  namespace = 'default',
  pollMs = 2000,
  copy,
}: ControlPlaneProps) {
  const [ns, setNs] = useState(namespace);
  const [projects, setProjects] = useState<PgProject[]>();
  const [projectId, setProjectId] = useState<string>();
  const [branches, setBranches] = useState<PgBranch[]>([]);
  const [branchId, setBranchId] = useState<string>();
  const [operations, setOperations] = useState<PgOperation[]>([]);
  const [error, setError] = useState<string>();
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const openIds = useRef(new Set<string>());
  // Bumped on a namespace (or client) change: a load that started under an older value
  // drops its result and its error rather than writing another namespace's data.
  const generation = useRef(0);
  // Bumped when an operation finishes, so the branch effect reloads with the projectId
  // that the project reload settled on, not the one in this render's closure.
  const [branchesRev, setBranchesRev] = useState(0);

  const project = projects?.find((p) => p.id === projectId);

  const loadProjects = useCallback(async () => {
    const gen = generation.current;
    try {
      const list = await client.listProjects(ns);
      if (gen !== generation.current) return;
      setProjects(list);
      setProjectId((cur) => (cur && list.some((p) => p.id === cur) ? cur : list[0]?.id));
      setError(undefined);
    } catch (e) {
      if (gen === generation.current) setError(failureText(e));
    }
  }, [client, ns]);

  const loadBranches = useCallback(async () => {
    if (!projectId) {
      setBranches([]);
      return;
    }
    const gen = generation.current;
    try {
      const list = await client.listBranches(ns, projectId);
      if (gen !== generation.current) return;
      setBranches(list);
      setBranchId((cur) =>
        cur && list.some((b) => b.id === cur)
          ? cur
          : (list.find((b) => b.isDefault) ?? list[0])?.id,
      );
    } catch (e) {
      if (gen === generation.current) setError(failureText(e));
    }
  }, [client, ns, projectId]);

  // Reload the lists when an operation the page saw open has finished.
  const loadOperations = useCallback(async () => {
    const gen = generation.current;
    try {
      const ops = await client.listOperations(ns);
      if (gen !== generation.current) return;
      setOperations(ops);
      let finished = false;
      for (const op of ops) {
        if (isOpen(op)) openIds.current.add(op.id);
        else if (openIds.current.delete(op.id)) finished = true;
      }
      if (finished) {
        await loadProjects();
        setBranchesRev((n) => n + 1);
      }
    } catch (e) {
      if (gen === generation.current) setError(failureText(e));
    }
  }, [client, ns, loadProjects]);

  // Only on a namespace (or client) change; loadOperations also changes with the project.
  // oxlint-disable-next-line react/exhaustive-deps -- reload per namespace
  useEffect(() => {
    generation.current += 1;
    setProjects(undefined);
    setProjectId(undefined);
    setBranches([]);
    setOperations([]);
    setError(undefined);
    openIds.current.clear();
    void loadProjects();
    void loadOperations();
  }, [ns, client]);

  // branchesRev is not read here: a bump is the signal to reload.
  // oxlint-disable-next-line react/exhaustive-deps -- reload when an operation finishes
  useEffect(() => {
    void loadBranches();
  }, [loadBranches, branchesRev]);

  const anyOpen = operations.some(isOpen);
  useEffect(() => {
    if (!anyOpen) return;
    const t = setInterval(() => void loadOperations(), pollMs);
    return () => clearInterval(t);
  }, [anyOpen, loadOperations, pollMs]);

  const track = (op: PgOperation) => {
    openIds.current.add(op.id);
    setOperations((ops) => [op, ...ops.filter((o) => o.id !== op.id)]);
  };

  const names = useMemo(() => {
    const out: Record<string, string> = {};
    for (const p of projects ?? []) out[p.id] = p.name;
    for (const b of branches) out[b.id] = b.name;
    return out;
  }, [projects, branches]);

  const selectedBranch = branches.find((b) => b.id === branchId);

  return (
    <div className="flex flex-col gap-4">
      {client.mock && (
        <SampleDataNotice>
          pg-control isn't reachable, so this is fixture data and nothing here changes a database.
          Set LOAMS_PG_CONTROL_API_KEY_FILE (and start pg-control) to connect.
        </SampleDataNotice>
      )}
      <div className="flex flex-wrap items-end gap-3">
        <NamespacePicker
          ns={ns}
          storageKey={NAMESPACES_KEY}
          idPrefix="postgres"
          onOpen={(next) => setNs(next)}
        />
        {projects && projects.length > 0 && (
          <Field label="Project">
            {(p) => (
              <Select {...p} value={projectId} onChange={(e) => setProjectId(e.target.value)}>
                {projects.map((x) => (
                  <option key={x.id} value={x.id}>
                    {x.name}
                  </option>
                ))}
              </Select>
            )}
          </Field>
        )}
        <Button variant="primary" onClick={() => setCreating(true)}>
          New project
        </Button>
        {project && (
          <Button
            variant="danger"
            disabled={project.state === 'deleting'}
            onClick={() => setDeleting(true)}
          >
            Delete project
          </Button>
        )}
      </div>

      {error && (
        <Notice tone="danger" title="The control plane refused a call">
          {error}
        </Notice>
      )}

      {projects === undefined ? (
        <p className="text-body text-muted">Loading projects…</p>
      ) : !project ? (
        <Empty title="No projects in this namespace">
          A project is a Postgres database with its own branches. Create one to start.
        </Empty>
      ) : (
        <>
          <section aria-labelledby="pg-project" className="flex flex-col gap-3">
            <h2 id="pg-project" className="text-section font-semibold">
              {project.name} <Badge>Postgres {project.pgVersion}</Badge>{' '}
              {project.state !== 'ready' && <Badge>{project.state}</Badge>}
            </h2>
            <p className="text-small text-muted">
              History retention {Math.round(project.historyRetentionSeconds / 86400)} days · region{' '}
              {project.region}
            </p>
            {branches.length === 0 ? (
              <Empty title="No branches yet">
                The project's main branch appears when the project is ready.
              </Empty>
            ) : (
              <BranchTree
                branches={branches}
                selected={branchId}
                onSelect={setBranchId}
                onCreate={async (input) => {
                  const r = await client.createBranch(ns, project.id, input);
                  track(r.operation);
                  setBranchId(r.branch.id);
                  await loadBranches();
                }}
                onProtect={async (b, value) => {
                  await client.setBranchProtected(ns, b, value);
                  await loadBranches();
                }}
                onDelete={async (b) => {
                  track(await client.deleteBranch(ns, b));
                  await loadBranches();
                }}
              />
            )}
          </section>
          {selectedBranch && selectedBranch.state !== 'deleting' && (
            <BranchAccess
              key={selectedBranch.id}
              client={client}
              namespace={ns}
              branch={selectedBranch}
              copy={copy}
            />
          )}
        </>
      )}

      <section aria-labelledby="pg-operations" className="flex flex-col gap-2">
        <h2 id="pg-operations" className="text-section font-semibold">
          Operations
        </h2>
        <Operations operations={operations} names={names} />
      </section>

      <CreateProjectDialog
        open={creating}
        onClose={() => setCreating(false)}
        onCreate={async (name, pgVersion) => {
          const r = await client.createProject(ns, { name, pgVersion });
          track(r.operation);
          setProjectId(r.project.id);
          await loadProjects();
        }}
      />
      {project && (
        <DeleteProjectDialog
          open={deleting}
          project={project}
          onClose={() => setDeleting(false)}
          onDelete={async () => {
            track(await client.deleteProject(ns, project));
            await loadProjects();
          }}
        />
      )}
    </div>
  );
}

function CreateProjectDialog({
  open,
  onClose,
  onCreate,
}: {
  open: boolean;
  onClose: () => void;
  onCreate: (name: string, pgVersion: number) => Promise<void>;
}) {
  const [name, setName] = useState('');
  const [version, setVersion] = useState<number>(PG_VERSIONS[0]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const close = () => {
    setName('');
    setError(undefined);
    onClose();
  };
  async function submit() {
    if (!name.trim()) return;
    setBusy(true);
    setError(undefined);
    try {
      await onCreate(name.trim(), version);
      close();
    } catch (e) {
      setError(failureText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open={open}
      onClose={close}
      title="New Postgres project"
      footer={
        <>
          <Button onClick={close}>Cancel</Button>
          <Button variant="primary" disabled={busy || !name.trim()} onClick={submit}>
            {busy ? 'Creating…' : 'Create project'}
          </Button>
        </>
      }
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="Project name">
          {(p) => <Input {...p} value={name} autoFocus onChange={(e) => setName(e.target.value)} />}
        </Field>
        <Field label="Postgres version">
          {(p) => (
            <Select {...p} value={version} onChange={(e) => setVersion(Number(e.target.value))}>
              {PG_VERSIONS.map((v) => (
                <option key={v} value={v}>
                  {v}
                </option>
              ))}
            </Select>
          )}
        </Field>
        {error && (
          <Notice tone="danger" title="Could not create the project">
            {error}
          </Notice>
        )}
      </form>
    </Dialog>
  );
}

function DeleteProjectDialog({
  open,
  project,
  onClose,
  onDelete,
}: {
  open: boolean;
  project: PgProject;
  onClose: () => void;
  onDelete: () => Promise<void>;
}) {
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const close = () => {
    setTyped('');
    setError(undefined);
    onClose();
  };
  async function submit() {
    if (typed !== project.name) return;
    setBusy(true);
    setError(undefined);
    try {
      await onDelete();
      close();
    } catch (e) {
      setError(failureText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open={open}
      onClose={close}
      title={`Delete project ${project.name}?`}
      footer={
        <>
          <Button onClick={close}>Cancel</Button>
          <Button variant="danger" disabled={busy || typed !== project.name} onClick={submit}>
            {busy ? 'Deleting…' : 'Delete project'}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="text-body">
          Every branch, endpoint, role and database in the project is removed. This cannot be
          undone.
        </p>
        <Field label={`Type ${project.name} to confirm`}>
          {(p) => (
            <Input
              {...p}
              value={typed}
              autoComplete="off"
              onChange={(e) => setTyped(e.target.value)}
            />
          )}
        </Field>
        {error && (
          <Notice tone="danger" title="Could not delete the project">
            {error}
          </Notice>
        )}
      </div>
    </Dialog>
  );
}
