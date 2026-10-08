// Settings › Agent providers: one card per provider with its base URL, model
// and API key. The key goes to the main process and never comes back: the
// field is never prefilled, and a stored key shows as "Saved — leave blank to keep".

import type { ChatProviderId, ChatProviderInfo, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Card, Checkbox, Field, Input, Notice, StatusTag } from '@loams/ui';
import { useCallback, useEffect, useState } from 'react';
import type { AgentStore } from './store.js';

const originOf = (url: string): string | undefined => {
  try {
    return new URL(url).origin;
  } catch {
    return undefined;
  }
};

type Outcome =
  | { kind: 'saved' }
  | { kind: 'key_required'; message: string }
  | { kind: 'error'; message: string }
  | { kind: 'test_ok'; model: string; ms: number }
  | { kind: 'test_failed'; message: string };

function ProviderCard({
  provider,
  desktop,
  onChanged,
}: {
  provider: ChatProviderInfo;
  desktop: LoamsDesktopApi;
  onChanged: () => void;
}) {
  const [baseUrl, setBaseUrl] = useState(provider.baseUrl);
  const [model, setModel] = useState(provider.model);
  const [apiKey, setApiKey] = useState('');
  const [fallback, setFallback] = useState(provider.fallback === true);
  const [busy, setBusy] = useState<'save' | 'test'>();
  const [outcome, setOutcome] = useState<Outcome>();
  const id: ChatProviderId = provider.id;

  // The saved values move when another save lands (or the origin change clears the key).
  useEffect(() => {
    setBaseUrl(provider.baseUrl);
    setModel(provider.model);
    setFallback(provider.fallback === true);
  }, [provider.baseUrl, provider.model, provider.fallback]);

  const dirty =
    baseUrl !== provider.baseUrl ||
    model !== provider.model ||
    apiKey !== '' ||
    fallback !== (provider.fallback === true);
  const originChanged =
    provider.needsKey &&
    provider.hasKey &&
    apiKey === '' &&
    originOf(baseUrl) !== originOf(provider.baseUrl);

  const save = async () => {
    setBusy('save');
    setOutcome(undefined);
    try {
      const r = await desktop.chat.configureProvider(id, {
        baseUrl: baseUrl.trim(),
        model: model.trim(),
        ...(apiKey ? { apiKey } : {}),
        ...(provider.kind === 'anthropic' ? { fallback } : {}),
      });
      if (r.ok) {
        setApiKey('');
        setOutcome({ kind: 'saved' });
      } else if (r.code === 'key_required') {
        setApiKey('');
        setOutcome({ kind: 'key_required', message: r.message });
      } else {
        setOutcome({ kind: 'error', message: r.message });
      }
    } catch (e) {
      setOutcome({ kind: 'error', message: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(undefined);
      onChanged();
    }
  };

  const test = async () => {
    setBusy('test');
    setOutcome(undefined);
    try {
      const r = await desktop.chat.testProvider(id);
      setOutcome(
        r.ok
          ? { kind: 'test_ok', model: r.value.model, ms: r.value.ms }
          : { kind: 'test_failed', message: r.message },
      );
    } catch (e) {
      setOutcome({ kind: 'test_failed', message: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(undefined);
    }
  };

  const keyHint = !provider.needsKey
    ? 'No key needed for a local server.'
    : provider.hasKey
      ? 'Kept on this computer and never shown again.'
      : 'Paste the key from your provider account.';

  return (
    <Card
      title={provider.label}
      headingLevel={3}
      actions={
        <StatusTag status={provider.configured ? 'done' : 'planned'}>
          {provider.configured ? 'Ready' : 'No key'}
        </StatusTag>
      }
    >
      <form
        className="flex flex-col gap-3"
        aria-label={`${provider.label} settings`}
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <Field label="Base URL">
          {(p) => (
            <Input
              {...p}
              className="font-mono"
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              spellCheck={false}
            />
          )}
        </Field>
        <Field label="Model">
          {(p) => (
            <Input
              {...p}
              className="font-mono"
              value={model}
              onChange={(e) => setModel(e.target.value)}
              spellCheck={false}
            />
          )}
        </Field>
        {provider.needsKey && (
          <Field label="API key" hint={keyHint}>
            {(p) => (
              <Input
                {...p}
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={apiKey}
                placeholder={provider.hasKey ? 'Saved — leave blank to keep' : ''}
                onChange={(e) => setApiKey(e.target.value)}
              />
            )}
          </Field>
        )}
        {!provider.needsKey && <p className="m-0 text-sm text-muted">{keyHint}</p>}
        {provider.kind === 'anthropic' && (
          <div className="flex flex-col gap-1">
            <Checkbox
              label="Allow server-side fallback to another model on refusal"
              checked={fallback}
              onChange={(e) => setFallback(e.target.checked)}
            />
            <p className="m-0 text-xs text-muted">
              Off by default. When on, a refused request may be answered by a different Claude model
              chosen by Anthropic, and that model's price applies. The chat tells you which model
              answered. This uses a beta header and is sent only to Anthropic's own server.
            </p>
          </div>
        )}
        {originChanged && (
          <Notice tone="warn" title="This is a different server">
            The stored key was entered for {originOf(provider.baseUrl)}. It is not sent to{' '}
            {originOf(baseUrl) ?? 'the new address'}: saving removes it, and you enter the key for
            the new server.
          </Notice>
        )}
        {outcome?.kind === 'key_required' && (
          <Notice tone="warn" title="Enter the key for this server">
            {outcome.message}
          </Notice>
        )}
        {outcome?.kind === 'error' && (
          <Notice tone="danger" title="Not saved">
            {outcome.message}
          </Notice>
        )}
        {outcome?.kind === 'saved' && (
          <Notice tone="success" title="Saved">
            {provider.needsKey && !provider.hasKey
              ? 'Add a key to start using it.'
              : 'Ready to use.'}
          </Notice>
        )}
        {outcome?.kind === 'test_ok' && (
          <Notice tone="success" title="The provider answered">
            {outcome.model} replied in {outcome.ms} ms.
          </Notice>
        )}
        {outcome?.kind === 'test_failed' && (
          <Notice tone="danger" title="The test failed">
            {outcome.message}
          </Notice>
        )}
        <div className="flex gap-2 items-center">
          <Button type="submit" variant="primary" disabled={!dirty || busy !== undefined}>
            {busy === 'save' ? 'Saving…' : 'Save'}
          </Button>
          <Button
            disabled={dirty || !provider.configured || busy !== undefined}
            title={dirty ? 'Save your changes first: the test uses the saved settings.' : undefined}
            onClick={() => void test()}
          >
            {busy === 'test' ? 'Testing…' : 'Test'}
          </Button>
        </div>
      </form>
    </Card>
  );
}

export function ProvidersSection({
  desktop,
  store,
}: {
  desktop: LoamsDesktopApi;
  store: AgentStore;
}) {
  const [providers, setProviders] = useState<ChatProviderInfo[]>();
  const [error, setError] = useState<string>();
  const load = useCallback(async () => {
    try {
      setProviders(await desktop.chat.providers());
      setError(undefined);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [desktop]);
  useEffect(() => {
    void load();
  }, [load]);
  const changed = useCallback(() => {
    void load();
    void store.refreshProviders();
  }, [load, store]);
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h2 className="text-lg font-medium m-0">Agent providers</h2>
        <p className="m-0 mt-1 text-sm text-muted">
          The agent calls the provider you choose, straight from this computer. Keys are kept by the
          app, never shown again, and sent only to the server they were entered for.
        </p>
      </div>
      {providers?.some((p) => !p.persistent) && (
        <Notice tone="warn" title="No system keyring">
          This computer has no keyring available, so keys last only until you quit the app.
        </Notice>
      )}
      {error && (
        <Notice tone="danger" title="Could not read the providers">
          {error}
        </Notice>
      )}
      {!providers && !error && <p className="text-muted">Reading the providers…</p>}
      {providers?.map((p) => (
        <ProviderCard key={p.id} provider={p} desktop={desktop} onChanged={changed} />
      ))}
    </div>
  );
}
