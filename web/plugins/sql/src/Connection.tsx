// The connection panel: host, port, user and database, a URI and a `mysql` command.
// The password is shown once, on the response that minted it (GetConnectionInfo or a
// rotation), and lives only in this component's state; it is never stored or logged.

import { Button, Notice } from '@loams/ui';
import { useEffect, useRef, useState } from 'react';
import {
  type ConnectionInfo,
  connectionUri,
  failureMessage,
  mysqlCommand,
  type SqlClient,
} from './client.js';

export interface ConnectionProps {
  client: SqlClient;
  namespace: string;
  databaseId: string;
  branchId: string;
  copy: (text: string) => void | Promise<void>;
}

type State =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; info: ConnectionInfo };

export function Connection({ client, namespace, databaseId, branchId, copy }: ConnectionProps) {
  const [s, setS] = useState<State>({ state: 'loading' });
  const [password, setPassword] = useState('');
  const [copied, setCopied] = useState('');
  const [busy, setBusy] = useState(false);

  // A password is returned only by the response that minted it. When that response
  // arrives stale (StrictMode re-runs the effect, or the branch changed mid-flight),
  // keep it per database and branch so the matching active load can still show it.
  // It is never applied to another branch.
  const unclaimed = useRef(new Map<string, string>());
  const active = useRef('');
  const settled = useRef(false);

  const load = async (rotate: boolean, live: () => boolean = () => true) => {
    const key = `${databaseId}/${branchId}`;
    setBusy(true);
    try {
      const info = await client.getConnectionInfo(namespace, databaseId, branchId, rotate);
      if (!live()) {
        if (info.password === '') return;
        if (active.current === key && settled.current) setPassword(info.password);
        else unclaimed.current.set(key, info.password);
        return;
      }
      const minted = info.password || unclaimed.current.get(key) || '';
      unclaimed.current.delete(key);
      settled.current = true;
      setS({ state: 'ready', info });
      setPassword(minted);
      setCopied('');
    } catch (e) {
      if (live()) setS({ state: 'error', message: failureMessage(e) });
    } finally {
      if (live()) setBusy(false);
    }
  };

  // oxlint-disable-next-line react/exhaustive-deps -- reload when the branch changes
  useEffect(() => {
    let live = true;
    active.current = `${databaseId}/${branchId}`;
    settled.current = false;
    setS({ state: 'loading' });
    setPassword('');
    void load(false, () => live);
    return () => {
      live = false;
    };
  }, [client, namespace, databaseId, branchId]);

  const doCopy = async (what: string, text: string) => {
    await copy(text);
    setCopied(what);
  };

  if (s.state === 'loading')
    return (
      <p className="text-muted" role="status">
        Loading connection details…
      </p>
    );
  if (s.state === 'error')
    return (
      <Notice tone="danger" title="Could not load connection details">
        {s.message}
      </Notice>
    );

  const { info } = s;
  const uri = connectionUri(info);
  const cmd = mysqlCommand(info);
  const rows: [string, string][] = [
    ['Host', info.host],
    ['Port', String(info.port)],
    ['User', info.user],
    ['Database', info.database],
    ['TLS', info.tlsRequired ? 'Required' : 'Optional'],
  ];

  return (
    <section aria-labelledby="sql-connect-title" className="flex flex-col gap-3">
      <h3 id="sql-connect-title" className="text-sm font-semibold">
        Connection
      </h3>
      <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1 text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className={/* mono-ok: id */ 'font-mono'}>{v}</dd>
          </div>
        ))}
      </dl>

      <section className="flex flex-col gap-1" aria-labelledby="sql-uri-label">
        <span className="text-muted text-xs" id="sql-uri-label">
          Connection string (password not included)
        </span>
        <div className="flex items-center gap-2">
          <code className="font-mono text-sm break-all">{uri}</code>
          <Button
            size="sm"
            onClick={() => void doCopy('uri', uri)}
            aria-label="Copy connection string"
          >
            Copy
          </Button>
        </div>
      </section>

      <section className="flex flex-col gap-1" aria-labelledby="sql-cmd-label">
        <span className="text-muted text-xs" id="sql-cmd-label">
          mysql client
        </span>
        <div className="flex items-center gap-2">
          <code className="font-mono text-sm break-all">{cmd}</code>
          <Button size="sm" onClick={() => void doCopy('cmd', cmd)} aria-label="Copy mysql command">
            Copy
          </Button>
        </div>
      </section>

      {password ? (
        <Notice tone="warn" title="Password: shown once">
          <div className="flex flex-col gap-2">
            <span>Copy it now. It is not shown again; rotate it to get a new one.</span>
            <div className="flex items-center gap-2">
              <code className="font-mono text-sm" data-testid="sql-password">
                {password}
              </code>
              <Button
                size="sm"
                onClick={() => void doCopy('password', password)}
                aria-label="Copy password"
              >
                Copy
              </Button>
              <Button
                size="sm"
                variant="quiet"
                onClick={() => {
                  unclaimed.current.delete(`${databaseId}/${branchId}`);
                  setPassword('');
                }}
              >
                Hide
              </Button>
            </div>
          </div>
        </Notice>
      ) : (
        <div className="flex items-center gap-2">
          <span className="text-muted text-sm">The password was shown when it was created.</span>
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void load(true)}>
            Rotate password
          </Button>
        </div>
      )}

      <p className="sr-only" role="status" aria-live="polite">
        {copied === 'uri'
          ? 'Connection string copied'
          : copied === 'cmd'
            ? 'Command copied'
            : copied === 'password'
              ? 'Password copied'
              : ''}
      </p>
    </section>
  );
}
