// The graphs list (§48 §18.2): ListGraphs in pages of 50, Create (an owned graph, its
// name and languages) and Delete (the graph's name typed to confirm). Create and Delete
// are admin actions. The page does not guess whether the caller may take them: it shows
// them until an admin call answers PERMISSION_DENIED, and hides them from then on.

import { Code } from '@connectrpc/connect';
import { graph } from '@loams/proto';
import { Badge, Button, Checkbox, Dialog, Field, Input, Notice } from '@loams/ui';
import { type FormEvent, useCallback, useEffect, useState } from 'react';
import { type GraphClient, type GraphFailure, type Language, toFailure } from './client.js';

const NAME_RULE = /^[a-z][a-z0-9_-]{0,62}$/;

export interface GraphListProps {
  client: GraphClient;
  namespace: string;
  selected?: string;
  onSelect: (g: graph.Graph | undefined) => void;
  canAdmin: boolean;
  /** An admin call answered PERMISSION_DENIED. */
  onDenied: () => void;
}

const STATE_TEXT: Partial<Record<graph.GraphState, string>> = {
  [graph.GraphState.OPENING]: 'Opening',
  [graph.GraphState.EVICTED]: 'Evicted',
  [graph.GraphState.RELOADING]: 'Reloading',
  [graph.GraphState.DELETING]: 'Deleting',
  [graph.GraphState.FAILED]: 'Failed',
};

export function GraphList(p: GraphListProps) {
  const { client, namespace, onSelect, onDenied } = p;
  const [graphs, setGraphs] = useState<graph.Graph[]>([]);
  const [next, setNext] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<GraphFailure>();
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<graph.Graph>();
  const [notice, setNotice] = useState<string>();

  const load = useCallback(
    async (token = '') => {
      setLoading(true);
      setError(undefined);
      try {
        const res = await client.listGraphs(namespace, token);
        setGraphs((prev) => (token ? [...prev, ...res.graphs] : res.graphs));
        setNext(res.nextPageToken);
      } catch (e) {
        setError(toFailure(e));
        if (!token) setGraphs([]);
      } finally {
        setLoading(false);
      }
    },
    [client, namespace],
  );

  useEffect(() => {
    void load();
  }, [load]);

  const denied = (f: GraphFailure) => {
    if (f.code !== Code.PermissionDenied) return false;
    onDenied();
    setNotice('You cannot create or delete graphs here. Ask an admin of this server.');
    return true;
  };

  return (
    <nav className="flex flex-col gap-2" aria-label="Graphs">
      <div className="flex items-center justify-between gap-2">
        <h2 className="m-0 text-base">Graphs</h2>
        {p.canAdmin && (
          <Button size="sm" onClick={() => setCreating(true)}>
            New graph
          </Button>
        )}
      </div>
      {notice && <Notice title={notice} />}
      {error && (
        <Notice tone="danger" title="Could not list graphs">
          {error.message}
        </Notice>
      )}
      {!loading && !error && graphs.length === 0 && (
        <p className="m-0 text-sm text-muted">No graphs in {namespace} yet.</p>
      )}
      <ul className="m-0 flex list-none flex-col gap-1 p-0">
        {graphs.map((g) => {
          const on = g.name === p.selected;
          const state = STATE_TEXT[g.state];
          return (
            <li key={g.name} className="flex items-center gap-1">
              <button
                type="button"
                aria-current={on ? 'true' : undefined}
                className={`flex min-w-0 flex-1 cursor-pointer flex-col items-start rounded-md border border-solid px-2 py-1 text-left font-sans ${
                  on ? 'border-accent bg-accent-soft' : 'border-transparent bg-transparent'
                }`}
                onClick={() => onSelect(g)}
              >
                <span className="flex items-center gap-2 font-mono text-sm text-ink">
                  {g.name}
                  {g.mode === graph.GraphMode.LINKED && <Badge>linked</Badge>}
                  {state && <Badge>{state}</Badge>}
                </span>
                {g.stats && (
                  <span className="text-xs text-muted">
                    {g.stats.nodes.toString()} nodes · {g.stats.edges.toString()} relationships
                  </span>
                )}
              </button>
              {p.canAdmin && (
                <Button
                  size="sm"
                  variant="quiet"
                  aria-label={`Delete ${g.name}`}
                  onClick={() => setDeleting(g)}
                >
                  Delete
                </Button>
              )}
            </li>
          );
        })}
      </ul>
      {loading && <p className="m-0 text-sm text-muted">Loading…</p>}
      {next && !loading && (
        <Button size="sm" variant="quiet" onClick={() => void load(next)}>
          Load more
        </Button>
      )}
      {creating && (
        <CreateDialog
          client={client}
          namespace={namespace}
          onClose={() => setCreating(false)}
          onDenied={(f) => {
            setCreating(false);
            denied(f);
          }}
          onCreated={async (g) => {
            setCreating(false);
            await load();
            onSelect(g);
          }}
        />
      )}
      {deleting && (
        <DeleteDialog
          client={client}
          target={deleting}
          onClose={() => setDeleting(undefined)}
          onDenied={(f) => {
            setDeleting(undefined);
            denied(f);
          }}
          onDeleted={async () => {
            const name = deleting.name;
            setDeleting(undefined);
            if (name === p.selected) onSelect(undefined);
            await load();
          }}
        />
      )}
    </nav>
  );
}

function CreateDialog({
  client,
  namespace,
  onClose,
  onDenied,
  onCreated,
}: {
  client: GraphClient;
  namespace: string;
  onClose: () => void;
  onDenied: (f: GraphFailure) => void;
  onCreated: (g: graph.Graph) => void;
}) {
  const [name, setName] = useState('');
  const [cypher, setCypher] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const valid = NAME_RULE.test(name);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!valid) return;
    setBusy(true);
    setError(undefined);
    const languages: Language[] = cypher ? ['gql', 'cypher'] : ['gql'];
    try {
      onCreated(await client.createGraph(namespace, name, languages));
    } catch (err) {
      const f = toFailure(err);
      if (f.code === Code.PermissionDenied) onDenied(f);
      else setError(f.message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onClose={onClose} title="New graph">
      <form className="flex flex-col gap-3" onSubmit={submit}>
        <p className="m-0 text-sm text-muted">
          An owned graph in <span className="font-mono">{namespace}</span>, written with GQL.
        </p>
        <Field
          label="Name"
          hint="Lower-case letters, digits, - and _, starting with a letter."
          error={name && !valid ? 'Not a valid graph name.' : error}
        >
          {(a) => (
            <Input
              {...a}
              className="font-mono"
              value={name}
              onChange={(e) => setName(e.target.value)}
              spellCheck={false}
              autoFocus
            />
          )}
        </Field>
        <Checkbox label="GQL" checked disabled />
        <Checkbox
          label="Also accept Cypher"
          checked={cypher}
          onChange={(e) => setCypher(e.target.checked)}
        />
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy || !valid}>
            Create
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function DeleteDialog({
  client,
  target,
  onClose,
  onDenied,
  onDeleted,
}: {
  client: GraphClient;
  target: graph.Graph;
  onClose: () => void;
  onDenied: (f: GraphFailure) => void;
  onDeleted: () => void;
}) {
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const match = typed === target.name;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!match) return;
    setBusy(true);
    setError(undefined);
    try {
      await client.deleteGraph(target.namespace, target.name);
      onDeleted();
    } catch (err) {
      const f = toFailure(err);
      if (f.code === Code.PermissionDenied) onDenied(f);
      else setError(f.message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onClose={onClose} title={`Delete ${target.name}`}>
      <form className="flex flex-col gap-3" onSubmit={submit}>
        <p className="m-0">
          This deletes the graph and everything in it. Type{' '}
          <strong className="font-mono">{target.name}</strong> to confirm.
        </p>
        <Field label="Graph name" error={error}>
          {(a) => (
            <Input
              {...a}
              className="font-mono"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              autoFocus
            />
          )}
        </Field>
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="danger" disabled={busy || !match}>
            Delete graph
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
