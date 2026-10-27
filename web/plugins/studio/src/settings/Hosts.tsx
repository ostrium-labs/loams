// Settings › Agentd hosts (§53 §6.3, D909; SD1 Task 7). A host is a
// `loams-agentd serve --remote` the person runs: Studio in the browser reaches it
// over TLS with their own short-lived access token for the audience `loams-agentd`.
// Here they register hosts (name, URL, an optional note of a self-signed
// certificate's fingerprint), pick the one Studio uses, and remove them. The list
// lives in this browser (`loams.studio.hosts`); links name a host by id only
// (`?host=<id>`).
//
// Until the console's sign-in issues agentd tokens (MT1), a host's token is pasted
// here in development, held in this tab's memory only, under a warning.

import type { AgentdHost } from '@loams/agentd-client/hosts';
import { Badge, Button, Card, Empty, Field, Input, Notice, Snippet } from '@loams/ui';
import { type FormEvent, useState, useSyncExternalStore } from 'react';
import type { StudioAgentd } from '../agentd.js';
import type { StudioHosts } from '../hosts/agentd.js';

// Port 443 on the tailnet address: the hosted console's CSP allows `https://*.ts.net`,
// which matches the default port only (docs/guides/studio/remote-agentd.md).
const SERVE =
  'loams-agentd serve --remote --listen <tailnet IP>:443 --tls-cert cert.pem --tls-key key.pem ' +
  '--issuer https://auth.example.com/application/o/loams/ --audience loams-agentd ' +
  '--subject <your sub> --allow-origin <this console> --config config.toml';

export function Hosts({ agentd }: { agentd: StudioAgentd }) {
  if (!agentd.hosts) {
    const host = agentd.host();
    return (
      <Card title="Agentd hosts">
        {host ? (
          <p className="m-0 flex items-center gap-2">
            <span className="text-ink">{host.name}</span>
            <Badge>connected</Badge>
          </p>
        ) : (
          <Empty title="No agentd hosts">
            Studio in the browser works with a <code>loams-agentd</code> you run. A host runs:
            <Snippet copy={false}>{SERVE}</Snippet>
          </Empty>
        )}
      </Card>
    );
  }
  return <HostRegistryCard hosts={agentd.hosts} />;
}

function HostRegistryCard({ hosts }: { hosts: StudioHosts }) {
  // Re-render on registry changes, and on selection or token changes.
  const list = useSyncExternalStore(hosts.registry.subscribe, hosts.registry.list);
  useSyncExternalStore(hosts.subscribe, hosts.version);
  const selected = hosts.selected();
  return (
    <Card title="Agentd hosts">
      <div className="flex flex-col gap-4">
        {hosts.pastedTokens && (
          <Notice tone="warn" title="Development sign-in">
            This console cannot get an agentd access token from your sign-in yet, so you paste one
            for each host. It stays in this tab's memory and is gone when you reload. Paste only a
            token you issued for yourself, for the audience <code>loams-agentd</code>.
          </Notice>
        )}
        {list.length === 0 ? (
          <Empty title="No agentd hosts" seed={7}>
            Run <code>loams-agentd</code> on a machine you control, reachable from this browser over
            TLS (a tailnet, or a port you open), then add it below:
            <Snippet copy={false}>{SERVE}</Snippet>
          </Empty>
        ) : (
          <ul className="m-0 flex list-none flex-col gap-3 p-0" aria-label="Registered hosts">
            {list.map((host) => (
              <HostRow
                key={host.id}
                host={host}
                hosts={hosts}
                selected={host.id === selected?.id}
              />
            ))}
          </ul>
        )}
        <AddHost hosts={hosts} />
      </div>
    </Card>
  );
}

function HostRow({
  host,
  hosts,
  selected,
}: {
  host: AgentdHost;
  hosts: StudioHosts;
  selected: boolean;
}) {
  const [token, setToken] = useState('');
  const tokenId = `token-${host.id}`;
  return (
    <li className="flex flex-col gap-2 border border-line p-3" aria-label={host.name}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-ink">{host.name}</span>
        {selected && <Badge>in use</Badge>}
        {hosts.hasToken(host.id) ? <Badge>signed in</Badge> : <Badge>no token</Badge>}
        <span className="ml-auto flex gap-2">
          {!selected && (
            <Button size="sm" variant="secondary" onClick={() => hosts.select(host.id)}>
              Use
            </Button>
          )}
          <Button size="sm" variant="quiet" onClick={() => hosts.registry.remove(host.id)}>
            Remove
          </Button>
        </span>
      </div>
      <code className="break-all text-muted">{host.url}</code>
      {host.fingerprint && (
        <span className="break-all text-muted">Certificate SHA-256: {host.fingerprint}</span>
      )}
      {hosts.pastedTokens && (
        <form
          className="flex items-end gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            hosts.setPastedToken(host.id, token);
            setToken('');
          }}
        >
          <label className="sr-only" htmlFor={tokenId}>
            Access token for {host.name}
          </label>
          <Input
            id={tokenId}
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder="Paste an access token"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
          <Button size="sm" type="submit" disabled={!token.trim()}>
            Set token
          </Button>
        </form>
      )}
    </li>
  );
}

function AddHost({ hosts }: { hosts: StudioHosts }) {
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [fingerprint, setFingerprint] = useState('');
  const [error, setError] = useState<{ field: 'url' | 'fingerprint'; message: string }>();
  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      const host = hosts.registry.add({ name, url, fingerprint });
      if (!hosts.selected()) hosts.select(host.id);
      setName('');
      setUrl('');
      setFingerprint('');
      setError(undefined);
    } catch (err) {
      const code = (err as { code?: string }).code;
      setError({
        field: code === 'invalid_fingerprint' ? 'fingerprint' : 'url',
        message: err instanceof Error ? err.message : String(err),
      });
    }
  };
  return (
    <form className="flex flex-col gap-3" onSubmit={submit} aria-label="Add an agentd host">
      <Field label="Name">
        {(p) => (
          <Input
            {...p}
            value={name}
            placeholder="Dev box"
            onChange={(e) => setName(e.target.value)}
          />
        )}
      </Field>
      <Field
        label="URL"
        hint="https:// (http:// only for localhost)"
        error={error?.field === 'url' ? error.message : undefined}
      >
        {(p) => (
          <Input
            {...p}
            value={url}
            inputMode="url"
            placeholder="https://devbox.example.ts.net"
            onChange={(e) => setUrl(e.target.value)}
          />
        )}
      </Field>
      <Field
        label="Certificate fingerprint (optional)"
        hint="For a self-signed certificate: its SHA-256, to compare with what the browser shows"
        error={error?.field === 'fingerprint' ? error.message : undefined}
      >
        {(p) => (
          <Input
            {...p}
            value={fingerprint}
            spellCheck={false}
            onChange={(e) => setFingerprint(e.target.value)}
          />
        )}
      </Field>
      <div>
        <Button type="submit" variant="primary" disabled={!url.trim()}>
          Add host
        </Button>
      </div>
    </form>
  );
}
