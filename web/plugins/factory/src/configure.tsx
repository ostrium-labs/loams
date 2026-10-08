import type { FactoryAppId, FactoryAppInfo, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Card, Field, Input, Notice } from '@loams/ui';
import { useEffect, useState } from 'react';
import { HealthPill, PageHead } from './model.js';

/** `/factory/:app/configure`: a form generated from the app's credential fields. */
export function ConfigurePage({
  desktop,
  app,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  app: FactoryAppId;
  navigate: (to: string) => void;
}) {
  const [info, setInfo] = useState<FactoryAppInfo | null>();
  const [url, setUrl] = useState('');
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [result, setResult] = useState<FactoryAppInfo>();

  useEffect(() => {
    let live = true;
    desktop.factory
      .list()
      .then((all) => {
        if (!live) return;
        const found = all.find((a) => a.id === app) ?? null;
        setInfo(found);
        setUrl((u) => u || found?.url || '');
        setValues((v) => ({ ...(found?.fields ?? {}), ...v }));
      })
      .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [desktop, app]);

  if (info === undefined) {
    return (
      <div className="lc-page">
        <p className="text-sm text-muted">{error ?? 'Loading…'}</p>
      </div>
    );
  }
  if (info === null) {
    return (
      <div className="lc-page">
        <Notice tone="danger" title={`Unknown app: ${app}`} />
      </div>
    );
  }

  const save = async () => {
    setBusy(true);
    setError(undefined);
    setResult(undefined);
    try {
      const fields: Record<string, string> = {};
      for (const [k, v] of Object.entries(values)) if (v.trim()) fields[k] = v.trim();
      const saved = await desktop.factory.configure(app, url.trim(), fields);
      if (!saved.ok) {
        setError(saved.message);
        return;
      }
      // Secrets are never kept in the form after they are sent.
      setValues((v) =>
        Object.fromEntries(
          Object.entries(v).filter(
            ([k]) => !info.credentialFields.find((f) => f.key === k)?.secret,
          ),
        ),
      );
      setResult(await desktop.factory.test(app));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const configured = info.health !== 'unconfigured';

  return (
    <div className="lc-page">
      <PageHead
        title={`Configure ${info.label}`}
        subtitle="Credentials are encrypted on this computer and never shown again."
        actions={
          <Button variant="quiet" size="sm" onClick={() => navigate('/factory')}>
            Back to Software Factory
          </Button>
        }
      />
      <div className="max-w-xl">
        <Card title="Connection" actions={<HealthPill health={(result ?? info).health} />}>
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <Field label="URL" hint="The base URL of the app, for example https://git.example.com">
              {(p) => (
                <Input
                  {...p}
                  type="url"
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  placeholder="https://"
                  autoComplete="off"
                />
              )}
            </Field>
            {info.credentialFields.map((f) => (
              <Field
                key={f.key}
                label={f.label}
                hint={
                  f.secret && configured ? 'Stored. Enter a new value to replace it.' : undefined
                }
              >
                {(p) => (
                  <Input
                    {...p}
                    type={f.secret ? 'password' : 'text'}
                    value={values[f.key] ?? ''}
                    onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
                    placeholder={f.secret && configured ? 'Saved — leave blank to keep' : undefined}
                    autoComplete={f.secret ? 'new-password' : 'off'}
                    spellCheck={false}
                  />
                )}
              </Field>
            ))}
            {error && <Notice tone="danger" title={error} />}
            {result && (
              <Notice
                tone={result.health === 'ok' ? 'success' : 'danger'}
                title={
                  result.health === 'ok'
                    ? 'Connected. The credentials work.'
                    : result.health === 'auth_failed'
                      ? 'The app rejected these credentials.'
                      : 'The app could not be reached.'
                }
              />
            )}
            <div className="flex flex-wrap gap-2">
              <Button type="submit" variant="primary" disabled={busy || !url.trim()}>
                {busy ? 'Saving…' : 'Save and test'}
              </Button>
              {configured && (
                <Button
                  variant="quiet"
                  disabled={busy}
                  onClick={() => {
                    void desktop.factory.remove(app).then(() => navigate('/factory'));
                  }}
                >
                  Remove credentials
                </Button>
              )}
            </div>
          </form>
        </Card>
      </div>
    </div>
  );
}
