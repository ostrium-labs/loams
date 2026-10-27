// The hosted console's screens before the console itself: sign-in, the
// first-run "Connect your engine" screen (engine and sign-in mode), and the
// screen for a deploy whose sign-in configuration is wrong.

import { Button, Field, Input, Logo, Notice } from '@loams/ui';
import { type FormEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import type { AuthMode } from '../runtime-config.js';
import { demoHref } from './demo.js';
import { checkHealth, type Health, isTailnetHost, parseEngineUrl } from './engine.js';

function Frame({ title, children }: { title: string; children: ReactNode }) {
  return (
    <main className="lc-gate">
      <div className="lc-gate-card">
        <div className="lc-gate-brand">
          <Logo />
        </div>
        <h1>{title}</h1>
        {children}
        <p className="lc-gate-demo">
          Just looking? <a href={demoHref(true)}>Open the console on sample data</a>: the pages run
          in this browser, with no engine and nothing to sign in to.
        </p>
      </div>
    </main>
  );
}

/** How each sign-in mode reads on the connect screen. */
export const MODE_TEXT: Record<AuthMode, { title: string; body: string }> = {
  tailscale: {
    title: 'Tailscale',
    body: "No sign-in. The engine knows you from this device's tailnet login.",
  },
  oidc: {
    title: 'Loams account',
    body: 'Sign in at auth.loams.dev; the engine checks the token it is given.',
  },
  apikey: {
    title: 'API key',
    body: "Enter an engine API key on every load; it stays in this tab's memory only.",
  },
};

/** Planned, and shown as such: no links, nothing to click. */
const COMING_SOON = [
  'Deploy this console to your own Vercel or Cloudflare account',
  'Register your own sign-in client at auth.loams.dev',
  'Headscale networks',
];

export function ComingSoon() {
  return (
    <section className="lc-gate-soon" aria-label="Coming soon">
      <h2>Coming soon</h2>
      <ul>
        {COMING_SOON.map((item) => (
          <li key={item} aria-disabled="true">
            <span>{item}</span>
            <span className="lc-gate-soon-tag">Coming soon</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** The deploy's config.json cannot be used (an unknown mode, oidc without a client). */
export function ConfigErrorScreen({ message }: { message: string }) {
  return (
    <Frame title="This console is not set up yet">
      <p className="lc-gate-lead">
        The console's sign-in configuration is wrong. Whoever deployed it needs to set{' '}
        <code>LOAMS_AUTH_MODE</code> to <code>tailscale</code>, <code>oidc</code> or{' '}
        <code>apikey</code> and deploy it again.
      </p>
      <Notice tone="danger" title="Sign-in is not configured">
        {message}
      </Notice>
    </Frame>
  );
}

export function SignInScreen({
  issuer,
  error,
  onSignIn,
  onOtherMode,
}: {
  issuer: string;
  error?: string;
  onSignIn: () => Promise<void>;
  /** Back to the connect screen, to pick Tailscale or an API key instead. */
  onOtherMode?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string>();
  const go = () => {
    setBusy(true);
    setFailed(undefined);
    onSignIn().catch((e: unknown) => {
      setBusy(false);
      setFailed(e instanceof Error ? e.message : String(e));
    });
  };
  const problem = failed ?? error;
  return (
    <Frame title="Sign in to Loams">
      <p className="lc-gate-lead">
        The console runs in your browser and talks to your own Loams engine. Sign in with your Loams
        account; the engine checks the token it is given.
      </p>
      {problem && (
        <Notice tone="danger" title="Sign-in did not complete">
          {problem}
        </Notice>
      )}
      <div className="lc-gate-actions">
        <Button variant="primary" onClick={go} disabled={busy}>
          {busy ? 'Opening sign-in…' : 'Sign in'}
        </Button>
        <span className="lc-gate-faint">via {new URL(issuer).host}</span>
        {onOtherMode && (
          <Button variant="quiet" onClick={onOtherMode}>
            Use another sign-in mode
          </Button>
        )}
      </div>
    </Frame>
  );
}

export interface ConnectResult {
  engine: string;
  /** How the person signs in to this engine. */
  mode: AuthMode;
  /** The engine API key, in apikey mode; held in memory only. */
  apiKey?: string;
}

export function ConnectScreen({
  initial,
  fixed = false,
  modes,
  initialMode,
  check = checkHealth,
  onConnect,
}: {
  /** A remembered (or fixed) engine: prefilled and tested at once. */
  initial?: string;
  /** The deploy fixes the engine (LOAMS_ENGINE_URL): the URL cannot be changed. */
  fixed?: boolean;
  /** The sign-in modes on offer, in order; one of them is picked. */
  modes: readonly AuthMode[];
  /** The mode picked at first: the remembered one, else the deploy's default. */
  initialMode: AuthMode;
  check?: (origin: string) => Promise<Health>;
  onConnect: (result: ConnectResult) => void;
}) {
  const [url, setUrl] = useState(initial ?? '');
  const [mode, setMode] = useState<AuthMode>(initialMode);
  const [apiKey, setApiKey] = useState('');
  const askApiKey = mode === 'apikey';
  const [problem, setProblem] = useState<string>();
  const [testing, setTesting] = useState(false);
  const [health, setHealth] = useState<{ origin: string; result: Health }>();

  const parsed = parseEngineUrl(url);
  const tested = health && parsed.ok && health.origin === parsed.origin ? health.result : undefined;

  const test = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!parsed.ok) {
      setProblem(parsed.problem);
      return;
    }
    setProblem(undefined);
    setTesting(true);
    const result = await check(parsed.origin);
    setTesting(false);
    setHealth({ origin: parsed.origin, result });
  };

  // Test a remembered engine straight away, once.
  const autoTested = useRef(false);
  useEffect(() => {
    if (!initial || autoTested.current) return;
    autoTested.current = true;
    void test();
  });

  const keyMissing = askApiKey && apiKey.trim() === '';
  const connect = () => {
    if (!parsed.ok || keyMissing) return;
    onConnect({ engine: parsed.origin, mode, apiKey: askApiKey ? apiKey.trim() : undefined });
  };

  return (
    <Frame title="Connect your engine">
      <p className="lc-gate-lead">
        Your Loams engine runs on your machine or your own VMs, never on the public internet. This
        browser reaches it over your tailnet, so this device needs Tailscale running and logged in
        to the same tailnet as the engine.
      </p>
      <form className="lc-gate-form" onSubmit={test}>
        <Field
          label="Engine URL"
          hint="The engine's tailnet address, for example https://loams.your-tailnet.ts.net"
          error={problem}
        >
          {(props) => (
            <Input
              {...props}
              // Text, not url: a bare tailnet name (no scheme) is fine here.
              type="text"
              inputMode="url"
              autoComplete="url"
              spellCheck={false}
              placeholder="https://loams.your-tailnet.ts.net"
              value={url}
              readOnly={fixed}
              onChange={(e) => setUrl(e.currentTarget.value)}
            />
          )}
        </Field>
        {modes.length > 1 && (
          <fieldset className="lc-gate-modes">
            <legend>Sign in with</legend>
            {modes.map((m) => (
              <label key={m} className="lc-gate-mode">
                <input
                  type="radio"
                  name="lc-auth-mode"
                  value={m}
                  checked={mode === m}
                  onChange={() => setMode(m)}
                />
                <span>
                  <strong>{MODE_TEXT[m].title}</strong>
                  {m === 'tailscale' && <span className="lc-gate-faint"> (default)</span>}
                  <span className="lc-gate-mode-body">{MODE_TEXT[m].body}</span>
                </span>
              </label>
            ))}
          </fieldset>
        )}
        {askApiKey && (
          <Field
            label="API key"
            hint="This console signs in to the engine with an API key. Kept in this tab's memory only, so it is asked for on every load."
          >
            {(props) => (
              <Input
                {...props}
                type="password"
                autoComplete="off"
                aria-required="true"
                value={apiKey}
                onChange={(e) => setApiKey(e.currentTarget.value)}
              />
            )}
          </Field>
        )}
        <div className="lc-gate-actions">
          <Button type="submit" disabled={testing || url.trim() === ''}>
            {testing ? 'Testing…' : 'Test connection'}
          </Button>
          <Button variant="primary" onClick={connect} disabled={!tested?.ok || keyMissing}>
            Connect
          </Button>
          {tested && !tested.ok && (
            <Button variant="quiet" onClick={connect} disabled={keyMissing}>
              Connect anyway
            </Button>
          )}
        </div>
      </form>
      {tested?.ok && (
        <Notice tone="success" title="The engine answered">
          /health returned {tested.status} in {tested.ms} ms.
        </Notice>
      )}
      {tested && !tested.ok && parsed.ok && (
        <Notice tone="danger" title="This browser cannot reach the engine">
          {/* The browser's own words for a network failure ("Failed to fetch") say nothing. */}
          {tested.kind !== 'unreachable' && <>{tested.message} </>}
          <Unreachable origin={parsed.origin} result={tested} />
        </Notice>
      )}
      <details className="lc-gate-help">
        <summary>How the engine is reached</summary>
        <ul>
          <li>
            The release Compose file (<code>deploy/release/compose.yaml</code>) runs the engine with
            a <code>tailscale</code> sidecar that joins your tailnet as <code>loams</code> and
            serves the engine over HTTPS with <code>tailscale serve</code>. Nothing listens on the
            host's public interfaces.
          </li>
          <li>
            Tailscale: install the client on this device, log in to the same tailnet, and use the
            MagicDNS name, <code>https://loams.&lt;tailnet&gt;.ts.net</code>.
          </li>
          <li>
            The engine must allow this console's origin (CORS) and accept your sign-in: see{' '}
            <code>LOAMS_AUTH_MODE</code> in the Compose <code>.env</code>.
          </li>
        </ul>
      </details>
      <ComingSoon />
    </Frame>
  );
}

function Unreachable({ origin, result }: { origin: string; result: Health }) {
  if (result.ok) return null;
  if (result.kind === 'http') return <>The engine is up but not healthy yet; try again shortly.</>;
  if (isTailnetHost(origin))
    return (
      <>
        Check that Tailscale is running on this device and logged in to the engine's tailnet, that
        the engine is up (<code>docker compose ps</code>), and that it allows this console's origin.
      </>
    );
  return (
    <>
      Check that this device can reach that address (VPN or tailnet up), that the engine is running,
      and that it allows this console's origin.
    </>
  );
}

/** While a remembered engine's /health is checked (up to a few seconds when it is down). */
export function ConnectingScreen({ engine }: { engine: string }) {
  return (
    <Frame title="Connecting">
      <p className="lc-gate-lead" role="status">
        Checking the engine at <code>{new URL(engine).host}</code>…
      </p>
    </Frame>
  );
}

export function BootError({ message }: { message: string }) {
  return (
    <Frame title="The console could not start">
      <Notice tone="danger" title="Error">
        {message}
      </Notice>
    </Frame>
  );
}
