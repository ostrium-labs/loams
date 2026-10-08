import type { SqlConnection } from '@loams/desktop/contracts';
import { Button, Card, Notice, Snippet } from '@loams/ui';
import { useEffect, useState } from 'react';

export type Dialect = 'postgres' | 'mysql';

const dots = '••••••••••••';

/** The one-line client command. The password is never part of it. */
export function clientCommand(d: Dialect, c: SqlConnection): string {
  return d === 'postgres'
    ? `psql "host=${c.host} port=${c.port} dbname=${c.database} user=${c.user}"`
    : `mysql -h ${c.host} -P ${c.port} -u ${c.user} -p ${c.database}`;
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[8rem_1fr] items-center gap-3 border-b border-rule-soft py-2 text-sm">
      <dt className="text-muted">{label}</dt>
      <dd className="m-0 flex items-center gap-2 font-mono">{children}</dd>
    </div>
  );
}

/** Host, port, database, user, a password hidden until revealed, and a client command. */
export function ConnectPanel({
  dialect,
  connection,
  revealPassword,
  copy,
}: {
  dialect: Dialect;
  connection: () => Promise<SqlConnection>;
  revealPassword: () => Promise<string>;
  copy: (text: string) => Promise<unknown>;
}) {
  const [conn, setConn] = useState<SqlConnection>();
  const [error, setError] = useState<string>();
  const [password, setPassword] = useState<string>();
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let live = true;
    connection()
      .then((c) => live && setConn(c))
      .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [connection]);

  async function reveal() {
    try {
      setPassword(await revealPassword());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function copyPassword() {
    try {
      await copy(password ?? (await revealPassword()));
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  if (error && !conn) {
    return (
      <Notice tone="danger" title="Could not read the connection">
        {error}
      </Notice>
    );
  }
  if (!conn) return <p className="text-sm text-muted">Loading…</p>;

  return (
    <div className="flex flex-col gap-4">
      <Card title="Connection">
        <dl className="m-0">
          <Row label="Host">{conn.host}</Row>
          <Row label="Port">{conn.port}</Row>
          <Row label="Database">{conn.database}</Row>
          <Row label="User">{conn.user}</Row>
          <Row label="Password">
            <span data-testid="password">{password ?? dots}</span>
            {password === undefined ? (
              <Button size="sm" onClick={reveal}>
                Reveal
              </Button>
            ) : (
              <Button size="sm" onClick={() => setPassword(undefined)}>
                Hide
              </Button>
            )}
            <Button size="sm" variant="quiet" aria-label="Copy password" onClick={copyPassword}>
              {copied ? 'Copied' : 'Copy'}
            </Button>
          </Row>
        </dl>
        {error && (
          <Notice tone="danger" title="Something went wrong">
            {error}
          </Notice>
        )}
      </Card>
      <Card title={dialect === 'postgres' ? 'psql' : 'mysql'}>
        <Snippet>{clientCommand(dialect, conn)}</Snippet>
        <p className="mb-0 mt-2 text-xs text-faint">The client asks for the password.</p>
      </Card>
    </div>
  );
}
