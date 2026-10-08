import { Badge, Button, Card, Empty, Field, Input, Select, Table, Textarea } from '@loams/ui';
import { type FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import {
  ApiError,
  OFFSET_OUT_OF_RANGE,
  type StreamDetail,
  type StreamsClient,
  type TailRecord,
} from '../client.js';
import { ErrorNotice, errorText, PageHead, useLoad } from '../shared.js';

export const TAIL_LIMIT = 500;
export const TAIL_POLL_MS = 2000;
export const TAIL_MAX_BACKOFF_MS = 30000;

/** Appends `incoming` to `kept`, keeping only the newest `limit`. */
export function capRecords(kept: TailRecord[], incoming: TailRecord[], limit = TAIL_LIMIT) {
  const all = kept.concat(incoming);
  return all.length > limit ? all.slice(all.length - limit) : all;
}

function preview(text: string): string {
  return text.length > 200 ? `${text.slice(0, 200)}…` : text;
}

/** The wait before the next poll: the base interval, doubled per consecutive failure, at most 30 s. */
export function backoffDelay(base: number, failures: number): number {
  return Math.min(TAIL_MAX_BACKOFF_MS, base * 2 ** Math.min(failures, 16));
}

/** Tails one partition from its high watermark: polls fetch, pausable, newest 500 kept. */
export function TailPanel({
  client,
  ns,
  stream,
  partition,
  pollMs = TAIL_POLL_MS,
}: {
  client: StreamsClient;
  ns: string;
  stream: string;
  partition: number;
  pollMs?: number;
}) {
  const [records, setRecords] = useState<TailRecord[]>([]);
  const [paused, setPaused] = useState(false);
  const [error, setError] = useState<string>();
  // The cursor outlives a pause, but belongs to one target: it is replaced, never mutated across targets.
  const cursorRef = useRef<{ key: string; offset?: string }>({ key: '' });

  useEffect(() => {
    if (paused) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    const key = `${ns}/${stream}/${partition}`;
    if (cursorRef.current.key !== key) {
      cursorRef.current = { key };
      setRecords([]);
      setError(undefined);
    }
    const cursor = cursorRef.current;
    const tick = async () => {
      try {
        if (cursor.offset === undefined) {
          const d = await client.describeStream(ns, stream);
          if (!live) return;
          cursor.offset =
            d.partitions.find((p) => p.partition === partition)?.high_watermark ?? '0';
        }
        const page = await client.fetch(ns, stream, partition, cursor.offset);
        if (!live) return;
        cursor.offset = page.nextOffset;
        failures = 0;
        setError(undefined);
        if (page.records.length > 0) setRecords((prev) => capRecords(prev, page.records));
      } catch (e) {
        if (!live) return;
        failures += 1;
        // Only an offset outside the log (trimmed) restarts at the watermark; a transient
        // failure keeps the cursor, so records produced meanwhile still arrive.
        if (e instanceof ApiError && e.code === OFFSET_OUT_OF_RANGE) cursor.offset = undefined;
        setError(errorText(e));
      }
      if (live) timer = setTimeout(tick, backoffDelay(pollMs, failures));
    };
    void tick();
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
    };
  }, [client, ns, stream, partition, paused, pollMs]);

  return (
    <Card
      title={`Tail, partition ${partition}`}
      actions={
        <div className="flex gap-2">
          <Button size="sm" onClick={() => setPaused((p) => !p)}>
            {paused ? 'Resume' : 'Pause'}
          </Button>
          <Button size="sm" variant="quiet" onClick={() => setRecords([])}>
            Clear
          </Button>
        </div>
      }
      flush
    >
      <p className="lc-muted px-4 pt-2 text-sm">
        {paused ? 'Paused.' : `Polling every ${pollMs / 1000} s.`} New records only, newest{' '}
        {TAIL_LIMIT} kept ({records.length} shown).
      </p>
      {error && (
        <div className="p-4">
          <ErrorNotice title="Could not fetch records" message={error} />
        </div>
      )}
      <Table<TailRecord>
        caption="Tail"
        rows={[...records].reverse()}
        rowKey={(r) => String(r.offset)}
        empty={<Empty title="Waiting for records">Produce a record and it appears here.</Empty>}
        columns={[
          { key: 'o', header: 'Offset', numeric: true, cell: (r) => <code>{r.offset}</code> },
          { key: 'k', header: 'Key', cell: (r) => (r.key == null ? '' : <code>{r.key}</code>) },
          { key: 'v', header: 'Value', cell: (r) => <code>{preview(r.value)}</code> },
        ]}
      />
    </Card>
  );
}

type Mode = 'json' | 'cloudevent';

const REQUIRED_EVENT_FIELDS = ['specversion', 'id', 'source', 'type'] as const;

/** Field-level problems of a structured CloudEvent, before it is posted. */
export function validateCloudEvent(event: unknown): string[] {
  if (typeof event !== 'object' || event === null || Array.isArray(event)) {
    return ['A CloudEvent must be a JSON object.'];
  }
  const e = event as Record<string, unknown>;
  const problems: string[] = [];
  for (const f of REQUIRED_EVENT_FIELDS) {
    if (typeof e[f] !== 'string' || (e[f] as string).trim() === '') {
      problems.push(`"${f}" is required and must be a non-empty string.`);
    }
  }
  if (typeof e.specversion === 'string' && e.specversion.trim() !== '' && e.specversion !== '1.0') {
    problems.push('"specversion" must be "1.0".');
  }
  return problems;
}

function ProduceCard({
  client,
  ns,
  stream,
  partitions,
  onProduced,
}: {
  client: StreamsClient;
  ns: string;
  stream: string;
  partitions: number;
  onProduced: () => void;
}) {
  const [mode, setMode] = useState<Mode>('json');
  const [partition, setPartition] = useState(0);
  const [key, setKey] = useState('');
  const [body, setBody] = useState('{"hello": "world"}');
  const [result, setResult] = useState<string>();
  const [error, setError] = useState<string>();
  const [problems, setProblems] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  // The stream may have fewer partitions than the last pick.
  const part = Math.min(partition, Math.max(0, partitions - 1));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    setProblems([]);
    setResult(undefined);
    setBusy(true);
    try {
      let parsed: unknown;
      try {
        parsed = JSON.parse(body);
      } catch {
        throw new Error('The body must be valid JSON.');
      }
      if (mode === 'json') {
        const ack = await client.produce(ns, stream, part, {
          key,
          value: JSON.stringify(parsed),
        });
        setResult(`Produced at offset ${ack.base_offset}.`);
      } else {
        const found = validateCloudEvent(parsed);
        if (found.length > 0) {
          setProblems(found);
          return;
        }
        const res = await client.produceEvent(ns, stream, part, parsed as Record<string, unknown>);
        const first = res.events[0];
        if (first?.status !== 'appended') {
          throw new Error(
            first
              ? `The event was not appended (status: ${first.status}).`
              : 'The server acknowledged no event.',
          );
        }
        setResult(`Event appended at offset ${first.offset ?? '?'}.`);
      }
      onProduced();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const switchMode = (m: Mode) => {
    setMode(m);
    setBody(
      m === 'json'
        ? '{"hello": "world"}'
        : JSON.stringify(
            {
              specversion: '1.0',
              id: `evt-${Date.now()}`,
              source: '/loams-desktop',
              type: 'dev.loams.test',
              datacontenttype: 'application/json',
              data: { hello: 'world' },
            },
            null,
            2,
          ),
    );
  };

  return (
    <Card title="Produce a test record">
      <form className="flex flex-col gap-3" onSubmit={submit}>
        <div className="flex flex-wrap gap-3">
          <Field label="Format">
            {(p) => (
              <Select {...p} value={mode} onChange={(e) => switchMode(e.target.value as Mode)}>
                <option value="json">JSON record</option>
                <option value="cloudevent">CloudEvent</option>
              </Select>
            )}
          </Field>
          <Field label="Partition">
            {(p) => (
              <Select
                {...p}
                value={String(part)}
                onChange={(e) => setPartition(Number(e.target.value))}
              >
                {Array.from({ length: partitions }, (_, i) => `p${i}`).map((id, i) => (
                  <option key={id} value={i}>
                    {i}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          {mode === 'json' && (
            <Field label="Key (optional)">
              {(p) => <Input {...p} value={key} onChange={(e) => setKey(e.target.value)} />}
            </Field>
          )}
        </div>
        <Field label={mode === 'json' ? 'Value (JSON)' : 'CloudEvent (JSON)'}>
          {(p) => (
            <Textarea
              {...p}
              rows={6}
              className="font-mono"
              value={body}
              onChange={(e) => setBody(e.target.value)}
            />
          )}
        </Field>
        <div className="flex items-center gap-3">
          <Button variant="primary" type="submit" disabled={busy}>
            Produce
          </Button>
          {result && <span className="text-sm text-muted">{result}</span>}
        </div>
        {problems.length > 0 && (
          <ErrorNotice title="The CloudEvent is not valid" message={problems.join(' ')} />
        )}
        {error && <ErrorNotice title="Could not produce the record" message={error} />}
      </form>
    </Card>
  );
}

export function StreamPage({
  client,
  ns,
  name,
  navigate,
}: {
  client: StreamsClient;
  ns: string;
  name: string;
  navigate: (to: string) => void;
}) {
  const [detail, reload] = useLoad<StreamDetail>(
    () => client.describeStream(ns, name),
    [client, ns, name],
  );
  const [tailPartition, setTailPartition] = useState(0);
  const refresh = useCallback(() => reload(), [reload]);
  const back = `/streams/${encodeURIComponent(ns)}`;
  const count = detail.state === 'ready' ? detail.data.partitions.length : 0;
  return (
    <div className="lc-page max-w-[1180px]">
      <PageHead
        crumbs={
          <>
            <a href={`#${back}`}>Streams & Links</a> / <code>{ns}</code>
          </>
        }
        title={<code>{name}</code>}
        actions={
          <>
            <Button onClick={() => navigate(back)}>Back</Button>
            <Button onClick={refresh}>Refresh</Button>
          </>
        }
      />
      {detail.state === 'loading' && <p className="lc-muted">Loading stream…</p>}
      {detail.state === 'error' && (
        <ErrorNotice title="Could not load the stream" message={detail.message} />
      )}
      {detail.state === 'ready' && (
        <>
          <Card title="Partitions" flush>
            <Table<StreamDetail['partitions'][number]>
              caption="Partitions"
              rows={detail.data.partitions}
              rowKey={(p) => String(p.partition)}
              empty={<Empty title="No partitions">This stream has no partitions.</Empty>}
              columns={[
                { key: 'p', header: 'Partition', numeric: true, cell: (p) => String(p.partition) },
                {
                  key: 's',
                  header: 'Log start offset',
                  numeric: true,
                  cell: (p) => <code>{p.log_start_offset}</code>,
                },
                {
                  key: 'h',
                  header: 'High watermark',
                  numeric: true,
                  cell: (p) => <code>{p.high_watermark}</code>,
                },
                {
                  key: 'n',
                  header: 'Records',
                  numeric: true,
                  cell: (p) => (
                    <Badge>{String(BigInt(p.high_watermark) - BigInt(p.log_start_offset))}</Badge>
                  ),
                },
              ]}
            />
          </Card>
          <ProduceCard
            client={client}
            ns={ns}
            stream={name}
            partitions={count}
            onProduced={refresh}
          />
          <div className="flex items-center gap-2">
            <label htmlFor="sp-tail-partition" className="text-sm text-muted">
              Tail partition
            </label>
            <Select
              id="sp-tail-partition"
              value={String(tailPartition)}
              onChange={(e) => setTailPartition(Number(e.target.value))}
            >
              {detail.data.partitions.map((p) => (
                <option key={p.partition} value={p.partition}>
                  {p.partition}
                </option>
              ))}
            </Select>
          </div>
          <TailPanel client={client} ns={ns} stream={name} partition={tailPartition} />
        </>
      )}
    </div>
  );
}
