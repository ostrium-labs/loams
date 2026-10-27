import type {
  LoamsDesktopApi,
  PgControlFieldErrors,
  PgControlSettings,
  PgControlTestResult,
} from '@loams/desktop/contracts';
import { parsePgControlUrl } from '@loams/desktop/pg-control';
import { Button, Card, Field, Input, Notice } from '@loams/ui';
import { useEffect, useState } from 'react';

type PgControlApi = NonNullable<LoamsDesktopApi['pgControl']>;

const KEY_SOURCE: Record<PgControlSettings['keySource'], string> = {
  env: 'with the key in LOAMS_PG_CONTROL_API_KEY',
  'env-file': 'with the key from the file LOAMS_PG_CONTROL_API_KEY_FILE names',
  'settings-file': 'with the key from the saved key file',
  none: 'without an API key',
};

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/**
 * Settings > Postgres: where pg-control's API listener is and which file holds its API key
 * (PG2 R9.1). Main reads the file and adds the key to each request; this page sees the path
 * only. Saving takes effect at once: the Postgres page probes pg-control again.
 */
export function PgControlSection({ api }: { api: PgControlApi }) {
  const [saved, setSaved] = useState<PgControlSettings>();
  const [url, setUrl] = useState('');
  const [keyFile, setKeyFile] = useState('');
  const [errors, setErrors] = useState<PgControlFieldErrors>({});
  const [busy, setBusy] = useState<'save' | 'test'>();
  const [test, setTest] = useState<PgControlTestResult>();
  const [note, setNote] = useState<{ tone: 'success' | 'danger'; text: string }>();

  useEffect(() => {
    let live = true;
    const take = (s: PgControlSettings) => {
      if (!live) return;
      setSaved(s);
      setUrl(s.url);
      setKeyFile(s.apiKeyFile);
      setErrors({});
    };
    api
      .settings()
      .then(take)
      .catch((e) => live && setNote({ tone: 'danger', text: message(e) }));
    const off = api.onChanged(take);
    return () => {
      live = false;
      off();
    };
  }, [api]);

  // The URL is checked as it is typed; the key file only by main, which can read it.
  const parsed = parsePgControlUrl(url);
  const urlError = parsed.ok ? errors.url : parsed.message;
  const input = { url, apiKeyFile: keyFile };
  const dirty = !!saved && (url.trim() !== saved.url || keyFile.trim() !== saved.apiKeyFile);

  const edit = (field: 'url' | 'apiKeyFile', v: string) => {
    (field === 'url' ? setUrl : setKeyFile)(v);
    setErrors(({ [field]: _gone, ...rest }) => rest);
    setTest(undefined);
    setNote(undefined);
  };

  const save = async () => {
    setBusy('save');
    setNote(undefined);
    try {
      const r = await api.save(input);
      if (r.ok) {
        setSaved(r.value);
        setUrl(r.value.url);
        setKeyFile(r.value.apiKeyFile);
        setErrors({});
        setNote({ tone: 'success', text: 'The Postgres page uses these settings now.' });
      } else {
        setErrors(r.errors ?? {});
        setNote({ tone: 'danger', text: r.message });
      }
    } catch (e) {
      setNote({ tone: 'danger', text: message(e) });
    } finally {
      setBusy(undefined);
    }
  };

  const runTest = async () => {
    setBusy('test');
    setTest(undefined);
    try {
      const r = await api.test(input);
      if (r.errors) setErrors(r.errors);
      setTest(r);
    } catch (e) {
      setTest({ ok: false, code: 'error', message: message(e), url: url || '' });
    } finally {
      setBusy(undefined);
    }
  };

  const choose = async () => {
    const path = await api.pickKeyFile().catch(() => null);
    if (path) edit('apiKeyFile', path);
  };

  const overrides = saved?.envOverrides ?? [];
  return (
    <div className="flex flex-col gap-4">
      <Card title="Postgres control plane">
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (dirty && !urlError) void save();
          }}
        >
          <p className="m-0 text-sm text-muted">
            The Postgres page manages projects, branches and roles through pg-control. These
            settings say where it listens and which file holds its API key.
          </p>
          <Field
            label="pg-control URL"
            hint={`Host and port of pg-control's API listener. Empty uses ${saved?.defaultUrl ?? 'the default'}.`}
            error={urlError}
          >
            {(p) => (
              <Input
                {...p}
                value={url}
                placeholder="127.0.0.1:7680"
                spellCheck={false}
                autoComplete="off"
                disabled={!saved}
                onChange={(e) => edit('url', e.target.value)}
              />
            )}
          </Field>
          <Field
            label="API key file"
            hint="The file pg-api-key wrote the key to. The app reads it when it calls pg-control; the key is never shown here. Leave empty for a listener that needs no key."
            error={errors.apiKeyFile}
          >
            {(p) => (
              <div className="flex gap-2">
                <Input
                  {...p}
                  className="min-w-0 flex-1"
                  value={keyFile}
                  placeholder="~/.config/loams/pg-api-key"
                  spellCheck={false}
                  autoComplete="off"
                  disabled={!saved}
                  onChange={(e) => edit('apiKeyFile', e.target.value)}
                />
                <Button disabled={!saved} onClick={() => void choose()}>
                  Choose file
                </Button>
              </div>
            )}
          </Field>
          {saved && (
            <p className="m-0 text-sm text-muted" data-testid="pg-control-effective">
              With the saved settings, requests go to <code>{saved.effectiveUrl}</code>{' '}
              {KEY_SOURCE[saved.keySource]}.
            </p>
          )}
          {overrides.length > 0 && (
            <Notice tone="warn" title="Set in the environment">
              {overrides.join(', ')} {overrides.length === 1 ? 'is' : 'are'} set for this app and
              {overrides.length === 1 ? ' wins' : ' win'} over these settings.
            </Notice>
          )}
          <div className="flex gap-2">
            <Button
              type="submit"
              variant="primary"
              disabled={!dirty || !!urlError || busy === 'test'}
              loading={busy === 'save'}
            >
              Save
            </Button>
            <Button
              disabled={!saved || !!urlError || busy === 'save'}
              loading={busy === 'test'}
              onClick={() => void runTest()}
            >
              Test connection
            </Button>
          </div>
          {test && (
            <Notice
              tone={test.ok ? 'success' : 'danger'}
              title={test.ok ? 'Connected' : 'Could not connect'}
            >
              {test.message}
            </Notice>
          )}
          {note && (
            <Notice tone={note.tone} title={note.tone === 'success' ? 'Saved' : 'Could not save'}>
              {note.text}
            </Notice>
          )}
        </form>
      </Card>
    </div>
  );
}
