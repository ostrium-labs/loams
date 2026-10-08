import type { LoamsDesktopApi, ServerEntry, ServerKind } from '@loams/desktop/contracts';
import { Badge, Button, Card, Empty, Field, Input, Notice, StatusTag, Table } from '@loams/ui';
import { type FormEvent, useState } from 'react';
import { EngineCard } from './engine-card.js';
import { useServers } from './state.js';

const KIND_LABEL: Record<ServerKind, string> = { local: 'Local', remote: 'Remote', demo: 'Demo' };

function AddServer({ desktop, onAdded }: { desktop: LoamsDesktopApi; onAdded(): void }) {
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const res = await desktop.servers.add({ name: name.trim(), url: url.trim(), kind: 'remote' });
      if (res.ok) {
        setName('');
        setUrl('');
        onAdded();
      } else {
        setError(res.message);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card title="Add a server">
      <form className="lc-server-form" onSubmit={submit}>
        <Field label="Name">
          {(p) => (
            <Input
              {...p}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Staging"
            />
          )}
        </Field>
        <Field
          label="URL"
          error={error}
          hint="The server's address, for example https://loams.example.com"
        >
          {(p) => (
            <Input
              {...p}
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://loams.example.com"
              inputMode="url"
            />
          )}
        </Field>
        <div>
          <Button type="submit" variant="primary" disabled={busy || !name.trim() || !url.trim()}>
            Add server
          </Button>
        </div>
      </form>
    </Card>
  );
}

/** `/settings/servers`: the registry, adding and removing, and the engine card. */
export function ServersPage({ desktop }: { desktop: LoamsDesktopApi }) {
  const { servers, activeId, loading, error, reload } = useServers(desktop);
  const [actionError, setActionError] = useState<string>();

  const run = async (op: () => Promise<{ ok: boolean; message?: string } | { ok: true }>) => {
    setActionError(undefined);
    try {
      const res = await op();
      if (!res.ok) setActionError((res as { message?: string }).message ?? 'The action failed.');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    }
    await reload();
  };

  return (
    <div className="lc-page">
      <header className="lc-page-head">
        <h1>Servers</h1>
        <p>The Loams servers this app can connect to. Switching reloads the window.</p>
      </header>
      {error && (
        <Notice tone="danger" title="Could not read the server list">
          {error}
        </Notice>
      )}
      {actionError && (
        <Notice tone="danger" title="That did not work">
          {actionError}
        </Notice>
      )}
      <Card title="Servers" flush>
        {loading ? (
          <p className="lc-muted" style={{ padding: 16 }}>
            Loading servers…
          </p>
        ) : (
          <Table<ServerEntry>
            caption="Servers"
            rows={servers}
            rowKey={(s) => s.id}
            empty={<Empty title="No servers">Add one below.</Empty>}
            columns={[
              { key: 'name', header: 'Name', cell: (s) => s.name },
              { key: 'kind', header: 'Kind', cell: (s) => <Badge>{KIND_LABEL[s.kind]}</Badge> },
              { key: 'url', header: 'URL', cell: (s) => <code>{s.url}</code> },
              {
                key: 'actions',
                header: '',
                cell: (s) => (
                  <span className="lc-row-actions">
                    {s.id === activeId ? (
                      <StatusTag status="done">Active</StatusTag>
                    ) : (
                      <Button
                        size="sm"
                        aria-label={`Activate ${s.name}`}
                        onClick={() => void run(() => desktop.servers.activate(s.id))}
                      >
                        Activate
                      </Button>
                    )}
                    {s.kind === 'remote' && (
                      <Button
                        size="sm"
                        variant="quiet"
                        aria-label={`Remove ${s.name}`}
                        onClick={() => void run(() => desktop.servers.remove(s.id))}
                      >
                        Remove
                      </Button>
                    )}
                  </span>
                ),
              },
            ]}
          />
        )}
      </Card>
      <AddServer desktop={desktop} onAdded={() => void reload()} />
      <EngineCard desktop={desktop} />
    </div>
  );
}
