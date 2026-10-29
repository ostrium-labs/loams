// `/pipelines`: the visual pipeline builder for CN2 connectors. A palette of sources and
// sinks by family plus transforms, a React Flow canvas with typed ports (an incompatible
// schema draws an error edge), a node side panel (secret fields write-only), validate and
// deploy, and once running, live throughput and lag per node from @loams/charts.

import { PageHead } from '@loams/desktop-ui';
import { Badge, Button, Card, Empty, Input, Notice, SampleDataNotice, StatusTag } from '@loams/ui';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Canvas, type Live } from './Canvas.js';
import { ConfigPanel } from './ConfigPanel.js';
import { type Deployment, failureMessage, type PipelinesClient } from './client.js';
import { type ConnectorDef, emptyGraph, type PipelineGraph, type Schema } from './model.js';
import { NodeMetrics } from './NodeMetrics.js';
import { addNode, connect, disconnect, moveNode, removeNodes, updateNode } from './ops.js';
import { Palette, type PaletteItem } from './Palette.js';
import { checkEdges, propagate, schemaText } from './schema.js';
import { autoLayout, secretRef, toPipelineSpec } from './spec.js';
import { type Issue, validateGraph } from './validate.js';

type Phase =
  | { state: 'editing' }
  | { state: 'validating' }
  | { state: 'deploying' }
  | { state: 'running'; deployment: Deployment }
  | { state: 'stopping'; deployment: Deployment };

type CatalogLoad =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; catalog: readonly ConnectorDef[] };

export interface PipelinesPageProps {
  client: PipelinesClient;
  /** The graph the page opens with; an empty one by default. */
  initial?: PipelineGraph;
  /** The client is a fixture: the page says so. */
  sample?: boolean;
}

const secretKey = (nodeId: string, key: string) => `${nodeId}/${key}`;

export function PipelinesPage({ client, initial, sample }: PipelinesPageProps) {
  const [load, setLoad] = useState<CatalogLoad>({ state: 'loading' });
  const [graph, setGraph] = useState<PipelineGraph>(initial ?? emptyGraph());
  const [selectedId, setSelectedId] = useState<string | undefined>();
  const [phase, setPhase] = useState<Phase>({ state: 'editing' });
  const [shownIssues, setShownIssues] = useState<readonly Issue[] | undefined>();
  const [error, setError] = useState<string | undefined>();
  const [announce, setAnnounce] = useState('');
  const [tick, setTick] = useState(0);
  // Secret values, write-only: kept only until the next successful deploy, never rendered,
  // never put in the graph or the spec.
  const secrets = useRef(new Map<string, string>());

  // oxlint-disable-next-line react/exhaustive-deps -- tick retries on purpose
  useEffect(() => {
    const abort = new AbortController();
    setLoad({ state: 'loading' });
    client
      .catalog(abort.signal)
      .then((catalog) => !abort.signal.aborted && setLoad({ state: 'ready', catalog }))
      .catch(
        (e) => !abort.signal.aborted && setLoad({ state: 'error', message: failureMessage(e) }),
      );
    return () => abort.abort();
  }, [client, tick]);

  const catalog = load.state === 'ready' ? load.catalog : [];
  const byRef = useMemo(() => new Map(catalog.map((c) => [c.id, c])), [catalog]);
  const lookup = useCallback((ref: string) => byRef.get(ref), [byRef]);

  const schemas = useMemo(() => propagate(graph, lookup), [graph, lookup]);
  const schemaTexts = useMemo(
    () => new Map(graph.nodes.map((n) => [n.id, schemaText(schemas.get(n.id))])),
    [graph.nodes, schemas],
  );
  const edgeChecks = useMemo(() => checkEdges(graph, lookup), [graph, lookup]);
  const liveIssues = useMemo(() => validateGraph(graph, lookup), [graph, lookup]);
  const problemsByNode = useMemo(() => {
    const m = new Map<string, string[]>();
    for (const i of liveIssues) {
      if (!i.nodeId || i.level !== 'error') continue;
      m.set(i.nodeId, [...(m.get(i.nodeId) ?? []), i.message]);
    }
    return m;
  }, [liveIssues]);

  const running = phase.state === 'running' || phase.state === 'stopping';
  const busy =
    phase.state === 'validating' || phase.state === 'deploying' || phase.state === 'stopping';
  const readOnly = running || busy;
  const deploymentId = running ? phase.deployment.id : undefined;
  const live: Live | undefined = useMemo(
    () => (deploymentId ? { client, deploymentId } : undefined),
    [client, deploymentId],
  );

  const edit = (next: PipelineGraph) => {
    setGraph(next);
    setShownIssues(undefined);
  };

  // Secret refs carry the pipeline name, so a rename leaves the store's values behind:
  // a node keeps a secret as set only while the page still holds a value to send.
  const rename = (name: string) => {
    if (name === graph.name) return;
    edit({
      ...graph,
      name,
      nodes: graph.nodes.map((n) => ({
        ...n,
        secrets: Object.fromEntries(
          Object.keys(n.secrets).map((k) => [k, secrets.current.has(secretKey(n.id, k))]),
        ),
      })),
    });
  };

  const onAdd = (item: PaletteItem, position?: { x: number; y: number }) => {
    if (readOnly) return;
    const r = addNode(graph, item, position);
    edit(r.graph);
    setSelectedId(r.id);
    setAnnounce(`Added ${item.label} ${item.kind}.`);
  };

  const onConnect = (source: string, target: string) => {
    const r = connect(graph, source, target);
    if (r.error) {
      setAnnounce(r.error);
      return;
    }
    edit(r.graph);
    const check = checkEdges(r.graph, lookup).get(`${source}->${target}`);
    const label = (id: string) => graph.nodes.find((n) => n.id === id)?.label ?? id;
    setAnnounce(
      check && !check.ok
        ? `Connected ${label(source)} to ${label(target)}, but the schemas do not match: ${check.problems.join(' ')}`
        : `Connected ${label(source)} to ${label(target)}.`,
    );
  };

  const onRemoveNodes = (ids: string[]) => {
    for (const id of ids) {
      for (const k of [...secrets.current.keys()])
        if (k.startsWith(`${id}/`)) secrets.current.delete(k);
    }
    edit(removeNodes(graph, ids));
    if (selectedId && ids.includes(selectedId)) setSelectedId(undefined);
    setAnnounce(ids.length === 1 ? 'Removed a node.' : `Removed ${ids.length} nodes.`);
  };

  const onSecret = (nodeId: string, key: string, value: string | undefined) => {
    const node = graph.nodes.find((n) => n.id === nodeId);
    if (!node) return;
    if (value === undefined) secrets.current.delete(secretKey(nodeId, key));
    else secrets.current.set(secretKey(nodeId, key), value);
    edit(updateNode(graph, nodeId, { secrets: { ...node.secrets, [key]: value !== undefined } }));
    setAnnounce(value === undefined ? 'Secret cleared.' : 'Secret saved.');
  };

  const validate = async (): Promise<boolean> => {
    setError(undefined);
    const local = validateGraph(graph, lookup);
    if (local.some((i) => i.level === 'error')) {
      setShownIssues(local);
      setAnnounce(`Validation found ${local.filter((i) => i.level === 'error').length} errors.`);
      return false;
    }
    setPhase({ state: 'validating' });
    try {
      const server = await client.validate(toPipelineSpec(graph));
      const all = [...local, ...server];
      setShownIssues(all);
      const errors = all.filter((i) => i.level === 'error').length;
      setAnnounce(errors ? `Validation found ${errors} errors.` : 'The pipeline is valid.');
      return errors === 0;
    } catch (e) {
      setError(failureMessage(e));
      return false;
    } finally {
      setPhase({ state: 'editing' });
    }
  };

  const deploy = async () => {
    if (!(await validate())) return;
    setPhase({ state: 'deploying' });
    const spec = toPipelineSpec(graph, { running: true });
    const values = new Map<string, string>();
    for (const n of graph.nodes) {
      for (const key of Object.keys(n.secrets)) {
        const v = secrets.current.get(secretKey(n.id, key));
        if (v !== undefined) values.set(secretRef(graph.name, n.id, key), v);
      }
    }
    try {
      const deployment = await client.deploy(spec, values);
      // Handed over: the store has them now, so the page forgets every value.
      secrets.current.clear();
      values.clear();
      setPhase({ state: 'running', deployment });
      setAnnounce(`Deployed ${deployment.name}, revision ${deployment.revision}. Running.`);
    } catch (e) {
      setPhase({ state: 'editing' });
      setError(failureMessage(e));
      setAnnounce('Deploy failed.');
    }
  };

  const stop = async () => {
    if (phase.state !== 'running') return;
    setPhase({ state: 'stopping', deployment: phase.deployment });
    try {
      await client.stop(phase.deployment.id);
      setPhase({ state: 'editing' });
      setAnnounce('Stopped. The pipeline can be edited again.');
    } catch (e) {
      setPhase({ state: 'running', deployment: phase.deployment });
      setError(failureMessage(e));
    }
  };

  const select = (id: string | undefined) => setSelectedId(id);
  const selected = graph.nodes.find((n) => n.id === selectedId);
  const inputOf = (id: string): Schema | undefined => {
    const ins = graph.edges.filter((e) => e.target === id).map((e) => schemas.get(e.source));
    if (ins.length === 0) return undefined;
    if (ins.length === 1) return ins[0];
    return ins.some((s) => !s || s.open)
      ? { open: true }
      : { fields: ins.flatMap((s) => (s && !s.open ? s.fields : [])) };
  };

  const status = running ? (
    <StatusTag status="done">{phase.state === 'stopping' ? 'Stopping' : 'Running'}</StatusTag>
  ) : busy ? (
    <StatusTag status="progress">
      {phase.state === 'validating' ? 'Validating' : 'Deploying'}
    </StatusTag>
  ) : (
    <StatusTag status="planned">Draft</StatusTag>
  );

  const actions = (
    <div className="flex flex-wrap items-center gap-2">
      {status}
      {!running && (
        <>
          <Button disabled={busy || graph.nodes.length < 2} onClick={() => edit(autoLayout(graph))}>
            Auto layout
          </Button>
          <Button disabled={busy} onClick={() => void validate()}>
            Validate
          </Button>
          <Button
            variant="primary"
            disabled={busy || load.state !== 'ready'}
            onClick={() => void deploy()}
          >
            Deploy
          </Button>
        </>
      )}
      {running && (
        <Button variant="danger" disabled={phase.state === 'stopping'} onClick={() => void stop()}>
          Stop
        </Button>
      )}
    </div>
  );

  const head = (
    <PageHead
      title="Pipelines"
      subtitle="Connect sources to sinks, with transforms between them."
      badge={<Badge>loams.flow/v1</Badge>}
      actions={actions}
    />
  );

  if (load.state === 'loading') {
    return (
      <div className="lc-page">
        {head}
        <p className="text-muted" role="status">
          Loading connectors…
        </p>
      </div>
    );
  }
  if (load.state === 'error') {
    return (
      <div className="lc-page">
        {head}
        <Notice tone="danger" title="The connector catalog did not load">
          {load.message}
        </Notice>
        <div>
          <Button onClick={() => setTick((t) => t + 1)}>Try again</Button>
        </div>
      </div>
    );
  }

  const issues = shownIssues ?? [];
  return (
    <div className="lc-page">
      {head}
      {sample && (
        <SampleDataNotice>
          Deploying here starts nothing; the run and its metrics are simulated.
        </SampleDataNotice>
      )}
      <p className="sr-only" role="status" aria-live="polite">
        {announce}
      </p>
      {error && (
        <Notice tone="danger" title="The server refused">
          {error}
        </Notice>
      )}
      {running && phase.deployment.message && (
        <Notice title="Deployed">{phase.deployment.message}</Notice>
      )}

      <div className="flex flex-wrap items-end gap-3">
        <div className="loams-field">
          <label htmlFor="pipeline-name">Pipeline name</label>
          <Input
            id="pipeline-name"
            className={/* mono-ok: id */ 'font-mono'}
            value={graph.name}
            disabled={readOnly}
            spellCheck={false}
            onChange={(e) => rename(e.target.value.toLowerCase())}
          />
        </div>
        <span className="text-body text-muted">
          {graph.nodes.length} nodes · {graph.edges.length} connections
          {liveIssues.some((i) => i.level === 'error') && !running ? ' · not ready to deploy' : ''}
        </span>
      </div>

      {shownIssues && (
        <section aria-label="Validation results">
          {issues.length === 0 ? (
            <Notice tone="success" title="Valid">
              The pipeline passes every check.
            </Notice>
          ) : (
            <Card title={`${issues.length} issue${issues.length > 1 ? 's' : ''}`}>
              <ul className="m-0 flex list-none flex-col gap-1 p-0 text-body">
                {issues.map((i) => (
                  <li
                    key={`${i.level}:${i.nodeId ?? ''}:${i.edgeId ?? ''}:${i.message}`}
                    className="flex items-baseline gap-2"
                  >
                    <span className={i.level === 'error' ? 'text-danger' : 'text-muted'}>
                      {i.level}
                    </span>
                    {i.nodeId ? (
                      <button
                        type="button"
                        className="cursor-pointer border-0 bg-transparent p-0 text-left text-ink underline"
                        onClick={() => select(i.nodeId)}
                      >
                        {i.message}
                      </button>
                    ) : (
                      <span>{i.message}</span>
                    )}
                  </li>
                ))}
              </ul>
            </Card>
          )}
        </section>
      )}

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-[220px_minmax(0,1fr)_340px]">
        <div className="lg:max-h-[70vh]">
          <Palette catalog={catalog} onAdd={(i) => onAdd(i)} disabled={readOnly} />
        </div>

        <div className="relative h-[70vh] min-w-0">
          <Canvas
            graph={graph}
            lookup={lookup}
            schemas={schemaTexts}
            edgeChecks={edgeChecks}
            problemsByNode={problemsByNode}
            selectedId={selectedId}
            live={live}
            readOnly={readOnly}
            onSelect={select}
            onMove={(id, p) => setGraph((g) => moveNode(g, id, p))}
            onConnect={onConnect}
            onRemoveNodes={onRemoveNodes}
            onRemoveEdges={(ids) => edit(disconnect(graph, ids))}
            onDropItem={(item, p) => onAdd(item, p)}
          />
          {graph.nodes.length === 0 && (
            <div className="pointer-events-none absolute inset-0 flex flex-col justify-center">
              <Empty title="No nodes yet">
                Add a source from the palette (click or drag it here), then a sink, and connect the
                source's output to the sink's input.
              </Empty>
            </div>
          )}
        </div>

        <div className="flex min-w-0 flex-col gap-4 lg:max-h-[70vh]">
          <nav aria-label="Pipeline outline" className="flex flex-col gap-2">
            <h2 className="m-0 text-small font-medium text-muted">Nodes</h2>
            {graph.nodes.length === 0 ? (
              <p className="m-0 text-body text-faint">None yet.</p>
            ) : (
              <ul className="m-0 flex list-none flex-col gap-1 p-0">
                {graph.nodes.map((n) => (
                  <li key={n.id}>
                    <button
                      type="button"
                      aria-current={n.id === selectedId ? 'true' : undefined}
                      onClick={() => select(n.id)}
                      className={`flex w-full items-center justify-between gap-2 border border-solid px-2 py-1 text-left text-body ${
                        n.id === selectedId
                          ? 'border-accent bg-accent-soft'
                          : 'border-rule bg-surface'
                      } text-ink`}
                    >
                      <span className="truncate">
                        {n.label} <span className="text-small text-faint">{n.kind}</span>
                      </span>
                      {problemsByNode.has(n.id) && (
                        <span className="text-small text-danger">
                          {problemsByNode.get(n.id)?.length} issue
                          {(problemsByNode.get(n.id)?.length ?? 0) > 1 ? 's' : ''}
                        </span>
                      )}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </nav>
          {selected ? (
            <ConfigPanel
              key={selected.id}
              graph={graph}
              node={selected}
              lookup={lookup}
              input={inputOf(selected.id)}
              output={schemas.get(selected.id)}
              problems={problemsByNode.get(selected.id) ?? []}
              readOnly={readOnly}
              onChange={(patch) => edit(updateNode(graph, selected.id, patch))}
              onSecret={(key, v) => onSecret(selected.id, key, v)}
              onConnect={onConnect}
              onDisconnect={(id) => edit(disconnect(graph, [id]))}
              onRemove={() => onRemoveNodes([selected.id])}
            />
          ) : (
            <p className="m-0 text-body text-muted">Select a node to configure it.</p>
          )}
        </div>
      </div>

      {running && live && (
        <section aria-label="Live metrics" className="flex flex-col gap-3">
          <h2 className="m-0 text-emphasis">Live metrics</h2>
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {graph.nodes.map((n) => (
              <NodeMetrics
                key={`${live.deploymentId}/${n.id}`}
                client={client}
                deploymentId={live.deploymentId}
                node={n}
              />
            ))}
          </div>
        </section>
      )}

      <details>
        <summary className="cursor-pointer text-body text-muted">Pipeline spec</summary>
        <pre className="mt-2 max-h-80 overflow-auto border border-solid border-rule bg-surface p-3 font-mono text-small">
          {JSON.stringify(toPipelineSpec(graph), null, 2)}
        </pre>
      </details>
    </div>
  );
}
