// `/graph`: the Graph page (§48 §18.2, D758). It asks the server whether it serves
// loams.graph.v1 (`GetInstance.services[]`, detect.ts) and shows one of three states:
// not served, not in this server's variant, or the page itself: the graphs list, the
// editor, the results and the schema sidebar. Every call goes over the console's
// Connect transport; there is no IPC channel and no main-process code.

import { Code, type Transport } from '@connectrpc/connect';
import { NamespacePicker, PageHead } from '@loams/desktop-ui';
import { graph } from '@loams/proto';
import { Badge, Button, Empty, Notice } from '@loams/ui';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  createGraphClient,
  type GraphFailure,
  type Language,
  type RunInput,
  toFailure,
} from './client.js';
import { GRAPH_PACKAGE, type GraphAvailability, graphAvailability } from './detect.js';
import { Editor } from './Editor.js';
import { GraphList } from './GraphList.js';
import { statementHistory } from './history.js';
import { type Result, Results, type RowsResult } from './Results.js';
import { Schema } from './Schema.js';
import { parametersFromJson } from './values.js';

export const DOCS_URL = 'https://loams.dev/docs';
const NAMESPACES_KEY = 'loams.graph.namespaces';

type Detected =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; availability: GraphAvailability; variant?: string; server: string };

export interface GraphPageProps {
  transport: Transport;
  /** Opens a link in the system browser. */
  openExternal: (url: string) => void;
  /** Where the history lives; the renderer's local storage by default. */
  storage?: Storage;
  /** The first namespace shown. */
  namespace?: string;
}

export function GraphPage({ transport, openExternal, storage, namespace }: GraphPageProps) {
  const client = useMemo(() => createGraphClient(transport), [transport]);
  const [detected, setDetected] = useState<Detected>({ state: 'loading' });
  const [tick, setTick] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: tick retries on purpose
  useEffect(() => {
    let live = true;
    setDetected({ state: 'loading' });
    (async () => {
      try {
        const info = await client.getInstance();
        const availability = graphAvailability(info);
        const variant = availability === 'not_in_variant' ? await client.notInVariant() : undefined;
        if (live) setDetected({ state: 'ready', availability, variant, server: info.instanceId });
      } catch (e) {
        if (live) setDetected({ state: 'error', message: toFailure(e).message });
      }
    })();
    return () => {
      live = false;
    };
  }, [client, tick]);

  const docs = (
    <Button variant="secondary" onClick={() => openExternal(DOCS_URL)}>
      Read the docs
    </Button>
  );
  const head = <PageHead title="Graph" badge={<Badge>{GRAPH_PACKAGE}</Badge>} />;

  if (detected.state === 'loading') {
    return (
      <div className="lc-page">
        {head}
        <p className="text-muted" role="status">
          Asking the server about Loams Graph…
        </p>
      </div>
    );
  }
  if (detected.state === 'error') {
    return (
      <div className="lc-page">
        {head}
        <Notice tone="danger" title="Could not reach the server">
          {detected.message}
        </Notice>
        <Button onClick={() => setTick((t) => t + 1)}>Try again</Button>
      </div>
    );
  }
  if (detected.availability === 'absent') {
    return (
      <div className="lc-page">
        {head}
        <Empty title="Graph is not served by this server" actions={docs} seed={21}>
          Loams Graph is GQL over {GRAPH_PACKAGE}. This server does not list it. Switch to a server
          that serves it, such as a loams built with the graph feature.
        </Empty>
      </div>
    );
  }
  if (detected.availability === 'not_in_variant') {
    return (
      <div className="lc-page">
        {head}
        <Empty title="Graph is not in this server's variant" actions={docs} seed={21}>
          {detected.variant
            ? `This server runs the ${detected.variant} variant, which does not include ${GRAPH_PACKAGE}.`
            : `This server's build does not include ${GRAPH_PACKAGE}.`}{' '}
          Run a build with the graph feature to use this page.
        </Empty>
      </div>
    );
  }
  return (
    <Workspace
      client={client}
      server={detected.server}
      storage={storage}
      initialNamespace={namespace ?? 'default'}
    />
  );
}

function Workspace({
  client,
  server,
  storage,
  initialNamespace,
}: {
  client: ReturnType<typeof createGraphClient>;
  server: string;
  storage?: Storage;
  initialNamespace: string;
}) {
  const history = useMemo(() => statementHistory(server, storage), [server, storage]);
  const [ns, setNs] = useState(initialNamespace);
  const [selected, setSelected] = useState<graph.Graph>();
  const [canAdmin, setCanAdmin] = useState(true);
  const [statement, setStatement] = useState('MATCH (n) RETURN n LIMIT 25');
  const [parameters, setParameters] = useState('');
  const [language, setLanguage] = useState<Language>('gql');
  const [readOnly, setReadOnly] = useState(true);
  const [busy, setBusy] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [result, setResult] = useState<Result>();
  const [ran, setRan] = useState<RunInput>();
  const [failure, setFailure] = useState<{ failure: GraphFailure; statement: string }>();
  const [status, setStatus] = useState<string>();
  const [recent, setRecent] = useState<string[]>(() => history.list());
  const [schemaTick, setSchemaTick] = useState(0);
  const abort = useRef<AbortController | undefined>(undefined);

  useEffect(() => setRecent(history.list()), [history]);
  useEffect(() => () => abort.current?.abort(), []);

  const cypher = selected?.languages.includes(graph.QueryLanguage.CYPHER) ?? false;

  const select = useCallback((g: graph.Graph | undefined) => {
    abort.current?.abort();
    setSelected(g);
    setResult(undefined);
    setFailure(undefined);
    setStatus(undefined);
    setLanguage('gql');
  }, []);

  /** Builds the call from the editor, or shows why it cannot. */
  const input = (): RunInput | undefined => {
    if (!selected) return undefined;
    try {
      return {
        namespace: ns,
        graph: selected.name,
        statement,
        language: cypher ? language : 'gql',
        parameters: parametersFromJson(parameters),
        readOnly,
      };
    } catch (e) {
      setFailure({
        failure: {
          code: Code.InvalidArgument,
          codeName: 'invalid_argument',
          message: (e as Error).message,
          metadata: {},
        },
        statement,
      });
      return undefined;
    }
  };

  const begin = () => {
    abort.current?.abort();
    const c = new AbortController();
    abort.current = c;
    setBusy(true);
    setFailure(undefined);
    setStatus(undefined);
    return c;
  };

  const fail = (c: AbortController, e: unknown, text: string) => {
    if (c.signal.aborted) {
      setStatus('Cancelled. The server stops the statement when the call goes away.');
      return;
    }
    setResult(undefined);
    setFailure({ failure: toFailure(e), statement: text });
  };

  const run = async () => {
    const call = input();
    if (!call) return;
    // The statement text only: parameters never reach the history (§48 §18.2).
    setRecent(history.add(call.statement));
    const c = begin();
    try {
      const res = await client.execute(call, c.signal);
      setRan(call);
      const rows: RowsResult = {
        kind: 'rows',
        columns: res.rows?.columns ?? [],
        columnTypes: res.rows?.columnTypes ?? [],
        rows: res.rows?.rows ?? [],
        truncated: res.truncated,
        counters: res.counters,
        notifications: res.notifications,
        elapsedNanos: res.elapsedNanos,
      };
      setResult(rows);
      if (res.counters && Object.values(res.counters).some((v) => typeof v === 'bigint' && v > 0n))
        setSchemaTick((t) => t + 1);
    } catch (e) {
      fail(c, e, call.statement);
    } finally {
      if (abort.current === c) setBusy(false);
    }
  };

  const explain = async (profile: boolean) => {
    const call = input();
    if (!call) return;
    const c = begin();
    try {
      const plan = await client.explain(call, profile, c.signal);
      setResult({ kind: 'plan', plan, profile });
    } catch (e) {
      fail(c, e, call.statement);
    } finally {
      if (abort.current === c) setBusy(false);
    }
  };

  const streamAll = async () => {
    if (!ran) return;
    const c = begin();
    setStreaming(true);
    const out: RowsResult = {
      kind: 'rows',
      columns: [],
      columnTypes: [],
      rows: [],
      truncated: false,
      notifications: [],
      elapsedNanos: 0n,
      streamed: true,
    };
    try {
      for await (const chunk of client.executeStream(ran, c.signal)) {
        if (chunk.columns.length) {
          out.columns = chunk.columns;
          out.columnTypes = chunk.columnTypes;
        }
        out.rows.push(...chunk.rows);
        if (chunk.last) {
          out.truncated = chunk.truncated;
          out.elapsedNanos = chunk.elapsedNanos;
          out.counters = chunk.counters;
          out.notifications = chunk.notifications;
        }
      }
      setResult({ ...out, rows: [...out.rows] });
    } catch (e) {
      fail(c, e, ran.statement);
    } finally {
      setStreaming(false);
      if (abort.current === c) setBusy(false);
    }
  };

  return (
    <div className="lc-page">
      <PageHead
        title="Graph"
        badge={<Badge>{GRAPH_PACKAGE}</Badge>}
        subtitle="Stored property graphs, queried with GQL."
        actions={
          <NamespacePicker
            ns={ns}
            storageKey={NAMESPACES_KEY}
            idPrefix="graph"
            onOpen={(next) => {
              setNs(next);
              select(undefined);
            }}
          />
        }
      />
      <div className="grid items-start gap-6 xl:grid-cols-[220px_minmax(0,1fr)_220px]">
        <GraphList
          key={ns}
          client={client}
          namespace={ns}
          selected={selected?.name}
          onSelect={select}
          canAdmin={canAdmin}
          onDenied={() => setCanAdmin(false)}
        />
        <div className="flex min-w-0 flex-col gap-4">
          {selected ? (
            <>
              <h2 className="m-0 text-base">
                <span className="font-mono">{selected.name}</span>
                <span className="ml-2 text-sm font-normal text-muted">in {ns}</span>
              </h2>
              <Editor
                statement={statement}
                onStatement={setStatement}
                parameters={parameters}
                onParameters={setParameters}
                language={language}
                onLanguage={setLanguage}
                cypher={cypher}
                readOnly={readOnly}
                onReadOnly={setReadOnly}
                busy={busy}
                onRun={() => void run()}
                onExplain={() => void explain(false)}
                onProfile={() => void explain(true)}
                onCancel={() => abort.current?.abort()}
                failure={failure}
                history={recent}
              />
              {status && <Notice title={status} />}
              {busy && !streaming && (
                <p className="m-0 text-sm text-muted" role="status">
                  Running…
                </p>
              )}
              {result && (
                <Results
                  result={result}
                  streaming={streaming}
                  onStreamAll={() => void streamAll()}
                />
              )}
            </>
          ) : (
            <Empty title="Pick a graph" seed={21}>
              Choose a graph on the left to query it, or create one.
            </Empty>
          )}
        </div>
        {selected && (
          <Schema client={client} namespace={ns} name={selected.name} refresh={schemaTick} />
        )}
      </div>
    </div>
  );
}
