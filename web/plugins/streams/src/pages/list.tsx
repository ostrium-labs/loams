import {
  Badge,
  Button,
  Card,
  Checkbox,
  Dialog,
  Empty,
  Field,
  Input,
  Select,
  StatusTag,
  Table,
  Textarea,
} from '@loams/ui';
import { type FormEvent, useState } from 'react';
import {
  isInternal,
  type LinkStatus,
  type LinkSummary,
  type StreamSummary,
  type StreamsClient,
} from '../client.js';
import {
  ErrorNotice,
  errorText,
  NamespacePicker,
  PageHead,
  rememberNamespace,
  useLoad,
} from '../shared.js';

export type ListTab = 'streams' | 'links';

const base = (ns: string) => `/streams/${encodeURIComponent(ns)}`;

/** The link status as the user reads it: `running` only means a target factory is registered. */
export function StatusBadge({ status }: { status: LinkStatus }) {
  return status === 'running' ? (
    <span title="A target factory is registered for this link's kind. This is not a liveness check.">
      <StatusTag status="done">Registered</StatusTag>
    </span>
  ) : (
    <span title="No factory is registered for this link's target kind, so nothing applies it.">
      <StatusTag status="progress">Unregistered</StatusTag>
    </span>
  );
}

function retentionText(s: StreamSummary): string {
  const r = s.retention;
  if (!r) return 'default';
  const parts: string[] = [];
  if (r.max_age_ms != null) parts.push(`${Math.round(r.max_age_ms / 1000)} s`);
  if (r.max_bytes != null) parts.push(`${r.max_bytes} B`);
  return parts.join(' / ') || 'default';
}

function optionalNumber(text: string, label: string): number | undefined {
  if (text.trim() === '') return undefined;
  const n = Number(text);
  if (!Number.isInteger(n) || n < 1) throw new Error(`${label} must be a positive whole number.`);
  return n;
}

function NewStream({
  client,
  ns,
  open,
  onClose,
  onCreated,
}: {
  client: StreamsClient;
  ns: string;
  open: boolean;
  onClose: () => void;
  onCreated: (name: string) => void;
}) {
  const [name, setName] = useState('');
  const [partitions, setPartitions] = useState('1');
  const [maxAge, setMaxAge] = useState('');
  const [maxBytes, setMaxBytes] = useState('');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    setBusy(true);
    try {
      const count = optionalNumber(partitions, 'Partitions') ?? 1;
      const max_age_ms = optionalNumber(maxAge, 'Retention age');
      const max_bytes = optionalNumber(maxBytes, 'Retention size');
      await client.createStream(ns, {
        name: name.trim(),
        partitions: count,
        ...(max_age_ms || max_bytes
          ? {
              retention: {
                ...(max_age_ms ? { max_age_ms: max_age_ms * 1000 } : {}),
                ...(max_bytes ? { max_bytes } : {}),
              },
            }
          : {}),
      });
      onCreated(name.trim());
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="New stream"
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            form="sp-new-stream"
            disabled={busy || !name.trim()}
          >
            Create stream
          </Button>
        </>
      }
    >
      <form id="sp-new-stream" className="flex flex-col gap-3" onSubmit={submit}>
        <Field label="Name">
          {(p) => (
            <Input
              {...p}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="orders"
            />
          )}
        </Field>
        <Field label="Partitions">
          {(p) => (
            <Input
              {...p}
              inputMode="numeric"
              value={partitions}
              onChange={(e) => setPartitions(e.target.value)}
            />
          )}
        </Field>
        <Field label="Retention age (seconds)" hint="Leave empty to keep records forever.">
          {(p) => (
            <Input
              {...p}
              inputMode="numeric"
              value={maxAge}
              onChange={(e) => setMaxAge(e.target.value)}
            />
          )}
        </Field>
        <Field label="Retention size (bytes)" hint="Leave empty for no size limit.">
          {(p) => (
            <Input
              {...p}
              inputMode="numeric"
              value={maxBytes}
              onChange={(e) => setMaxBytes(e.target.value)}
            />
          )}
        </Field>
        {error && <ErrorNotice title="Could not create the stream" message={error} />}
      </form>
    </Dialog>
  );
}

function parseOptions(text: string): Record<string, string> {
  if (text.trim() === '') return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error('Options must be valid JSON.');
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error('Options must be a JSON object of strings.');
  }
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(parsed)) {
    out[k] = typeof v === 'string' ? v : JSON.stringify(v);
  }
  return out;
}

function NewLink({
  client,
  ns,
  streams,
  open,
  onClose,
  onCreated,
}: {
  client: StreamsClient;
  ns: string;
  streams: string[];
  open: boolean;
  onClose: () => void;
  onCreated: (name: string) => void;
}) {
  const [name, setName] = useState('');
  const [source, setSource] = useState('');
  const [kind, setKind] = useState('counter');
  const [target, setTarget] = useState('');
  const [options, setOptions] = useState('');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const chosen = source || streams[0] || '';

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    setBusy(true);
    try {
      await client.createLink(ns, {
        name: name.trim(),
        source: chosen,
        target: { kind: kind.trim(), name: target.trim() || name.trim() },
        options: parseOptions(options),
      });
      onCreated(name.trim());
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="New link"
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            form="sp-new-link"
            disabled={busy || !name.trim() || !chosen}
          >
            Create link
          </Button>
        </>
      }
    >
      <form id="sp-new-link" className="flex flex-col gap-3" onSubmit={submit}>
        <Field label="Link name">
          {(p) => <Input {...p} value={name} onChange={(e) => setName(e.target.value)} />}
        </Field>
        <Field label="Source stream">
          {(p) => (
            <Select {...p} value={chosen} onChange={(e) => setSource(e.target.value)}>
              {streams.map((s) => (
                <option key={s}>{s}</option>
              ))}
            </Select>
          )}
        </Field>
        <Field
          label="Target kind"
          hint="counter is built in. A collection's own link is made with the collection."
        >
          {(p) => <Input {...p} value={kind} onChange={(e) => setKind(e.target.value)} />}
        </Field>
        <Field label="Target name" hint="Defaults to the link name.">
          {(p) => <Input {...p} value={target} onChange={(e) => setTarget(e.target.value)} />}
        </Field>
        <Field label="Options (JSON)" hint='For example {"key": "value"}. Leave empty for none.'>
          {(p) => (
            <Textarea
              {...p}
              rows={3}
              className="font-mono"
              value={options}
              onChange={(e) => setOptions(e.target.value)}
            />
          )}
        </Field>
        {error && <ErrorNotice title="Could not create the link" message={error} />}
      </form>
    </Dialog>
  );
}

/** `/streams` and `/streams/:ns[/links]`: the streams or the links of one namespace. */
export function ListPage({
  client,
  ns,
  tab,
  navigate,
}: {
  client: StreamsClient;
  ns: string;
  tab: ListTab;
  navigate: (to: string) => void;
}) {
  const [streams, reloadStreams] = useLoad(() => client.listStreams(ns), [client, ns]);
  const [links, reloadLinks] = useLoad(() => client.listLinks(ns), [client, ns]);
  const [showInternal, setShowInternal] = useState(false);
  const [creating, setCreating] = useState(false);
  if (streams.state === 'ready') rememberNamespace(ns);

  const visibleStreams =
    streams.state === 'ready'
      ? streams.data.filter((s) => showInternal || !isInternal(s.name))
      : [];
  const visibleLinks =
    links.state === 'ready' ? links.data.filter((l) => showInternal || !isInternal(l.name)) : [];
  const userStreams =
    streams.state === 'ready'
      ? streams.data.filter((s) => !isInternal(s.name)).map((s) => s.name)
      : [];
  const sourceNames =
    streams.state === 'ready' ? (showInternal ? streams.data.map((s) => s.name) : userStreams) : [];
  const active = tab === 'streams' ? streams : links;

  return (
    <div className="lc-page max-w-[1180px]">
      <PageHead
        title="Streams & Links"
        subtitle="Append-only logs and the links that apply them to a target."
        actions={
          <Button variant="primary" onClick={() => setCreating(true)}>
            {tab === 'streams' ? 'New stream' : 'New link'}
          </Button>
        }
      />
      <div className="flex flex-wrap items-end justify-between gap-4 border-b border-rule">
        <NamespacePicker
          key={ns}
          ns={ns}
          onOpen={(next) => navigate(base(next) + (tab === 'links' ? '/links' : ''))}
        />
        <div className="flex gap-1" role="tablist" aria-label="Streams and links">
          {(['streams', 'links'] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={t === tab}
              className="ds-tab"
              onClick={() => navigate(base(ns) + (t === 'links' ? '/links' : ''))}
            >
              {t === 'streams' ? 'Streams' : 'Links'}
            </button>
          ))}
        </div>
      </div>
      <div className="flex items-center justify-between">
        <Checkbox
          label="Show internal"
          checked={showInternal}
          onChange={(e) => setShowInternal(e.target.checked)}
        />
      </div>
      <Card title={tab === 'streams' ? `Streams in ${ns}` : `Links in ${ns}`} flush>
        {active.state === 'loading' && <p className="lc-muted p-4">Loading…</p>}
        {active.state === 'error' && (
          <div className="p-4">
            <ErrorNotice
              title={tab === 'streams' ? 'Could not list streams' : 'Could not list links'}
              message={active.message}
            />
          </div>
        )}
        {tab === 'streams' && streams.state === 'ready' && (
          <Table<StreamSummary>
            caption="Streams"
            rows={visibleStreams}
            rowKey={(s) => s.name}
            onRowClick={(s) => navigate(`${base(ns)}/stream/${encodeURIComponent(s.name)}`)}
            empty={
              <Empty title="No streams yet">Create a stream, then produce a record to it.</Empty>
            }
            columns={[
              {
                key: 'name',
                header: 'Name',
                cell: (s) => (
                  <a className="ds-link" href={`#${base(ns)}/stream/${encodeURIComponent(s.name)}`}>
                    <code>{s.name}</code>
                  </a>
                ),
              },
              {
                key: 'parts',
                header: 'Partitions',
                numeric: true,
                cell: (s) => String(s.partitions),
              },
              { key: 'ret', header: 'Retention', cell: (s) => <Badge>{retentionText(s)}</Badge> },
            ]}
          />
        )}
        {tab === 'links' && links.state === 'ready' && (
          <Table<LinkSummary>
            caption="Links"
            rows={visibleLinks}
            rowKey={(l) => l.name}
            onRowClick={(l) => navigate(`${base(ns)}/link/${encodeURIComponent(l.name)}`)}
            empty={<Empty title="No links yet">Create a link from a stream to a target.</Empty>}
            columns={[
              {
                key: 'name',
                header: 'Name',
                cell: (l) => (
                  <a className="ds-link" href={`#${base(ns)}/link/${encodeURIComponent(l.name)}`}>
                    <code>{l.name}</code>
                  </a>
                ),
              },
              { key: 'src', header: 'Source', cell: (l) => <code>{l.source ?? 'gone'}</code> },
              {
                key: 'tgt',
                header: 'Target',
                cell: (l) => (
                  <code>
                    {l.target.kind}/{l.target.name}
                  </code>
                ),
              },
              { key: 'status', header: 'Status', cell: (l) => <StatusBadge status={l.status} /> },
            ]}
          />
        )}
      </Card>
      {tab === 'streams' ? (
        <NewStream
          client={client}
          ns={ns}
          open={creating}
          onClose={() => setCreating(false)}
          onCreated={(name) => {
            setCreating(false);
            reloadStreams();
            navigate(`${base(ns)}/stream/${encodeURIComponent(name)}`);
          }}
        />
      ) : (
        <NewLink
          client={client}
          ns={ns}
          streams={sourceNames}
          open={creating}
          onClose={() => setCreating(false)}
          onCreated={(name) => {
            setCreating(false);
            reloadLinks();
            navigate(`${base(ns)}/link/${encodeURIComponent(name)}`);
          }}
        />
      )}
    </div>
  );
}
