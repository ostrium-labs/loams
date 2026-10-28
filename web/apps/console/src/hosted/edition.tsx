// The browser edition's plugin set (hosted console and BYOC deploys): the base
// catalog, web.yml (pages the engine serves), demo.yml under "Demo data", and a
// small header plugin that shows the connected engine, the toggle and sign-out.
// Demo mode (`?demo`) instead mounts DemoHeader: there is no engine to show.

import {
  type CatalogPatch,
  type ModuleTable,
  type PluginModule,
  parsePatch,
  service,
} from '@loams/console-host';
import type { Context } from '@loams/cordis';
import billingPkg from '@loams/plugin-billing/package.json';
import dataStudioPkg from '@loams/plugin-data-studio/package.json';
import durablePkg from '@loams/plugin-durable/package.json';
import factoryPkg from '@loams/plugin-factory/package.json';
import gitPkg from '@loams/plugin-git/package.json';
import graphPkg from '@loams/plugin-graph/package.json';
import housePkg from '@loams/plugin-house/package.json';
import livePkg from '@loams/plugin-live/package.json';
import metricsPkg from '@loams/plugin-metrics/package.json';
import pipelinesPkg from '@loams/plugin-pipelines/package.json';
import sqlPkg from '@loams/plugin-sql/package.json';
import streamsPkg from '@loams/plugin-streams/package.json';
import { Button } from '@loams/ui';
import { useEffect, useState } from 'react';
import demoYml from '../../catalog/demo.yml?raw';
import webYml from '../../catalog/web.yml?raw';
import type { AuthMode } from '../runtime-config.js';
import { DEMO_PAGES, demoHref } from './demo.js';
import { type EngineStatus, watchHealth } from './engine.js';

export const webModules: ModuleTable = {
  '@loams/plugin-live': () => import('@loams/plugin-live'),
  '@loams/plugin-graph': () => import('@loams/plugin-graph'),
  '@loams/plugin-house': () => import('@loams/plugin-house'),
  '@loams/plugin-sql': () => import('@loams/plugin-sql'),
  '@loams/plugin-metrics': () => import('@loams/plugin-metrics'),
  '@loams/plugin-pipelines': () => import('@loams/plugin-pipelines'),
  '@loams/plugin-git': () => import('@loams/plugin-git'),
  '@loams/plugin-data-studio': () => import('@loams/plugin-data-studio'),
  '@loams/plugin-streams': () => import('@loams/plugin-streams'),
  '@loams/plugin-durable': () => import('@loams/plugin-durable'),
  '@loams/plugin-factory': () => import('@loams/plugin-factory'),
  '@loams/plugin-billing': () => import('@loams/plugin-billing'),
};

export const webManifests: unknown[] = [
  livePkg,
  graphPkg,
  housePkg,
  sqlPkg,
  metricsPkg,
  pipelinesPkg,
  gitPkg,
  dataStudioPkg,
  streamsPkg,
  durablePkg,
  factoryPkg,
  billingPkg,
];

/** The patches over base.yml, in order. */
export function webPatches(demo: boolean): CatalogPatch[] {
  return demo ? [parsePatch(webYml), parsePatch(demoYml)] : [parsePatch(webYml)];
}

/**
 * The engine-served edition's patch (the console at the engine's `/ui/`, and the dev
 * server): web.yml's pages the engine serves (Data explorer, Streams & Links, Live,
 * Graph, Analytics), without web.yml's changes to base.yml's own rows.
 */
export function servedPagesPatch(): CatalogPatch {
  return parsePatch(webYml).filter((row) => 'insert' in row);
}

/** What the header shows and does; the hosted gate provides it. */
export interface HostedSession {
  engine: string;
  /** How this console signs in (LOAMS_AUTH_MODE). */
  authMode: AuthMode;
  /** Who the engine is talking to: the OIDC user, or the tailnet user the engine named. */
  user?: string;
  /** Tailscale mode: why the engine did not say who this is. */
  identityProblem?: string;
  /**
   * The engine runs with sign-in off (LOAMS_AUTH_MODE=off, loopback only): every caller
   * is the local developer, whatever the console's mode says.
   */
  authOff?: boolean;
  demo: boolean;
  setDemo(on: boolean): void;
  /** Back to the connect screen; unset when the deploy fixes the engine. */
  changeEngine?: () => void;
  signOut?: () => void;
}

export const HOSTED_PACKAGE = '@loams/console-hosted';

export const hostedManifest = {
  name: HOSTED_PACKAGE,
  version: '0.1.0',
  loams: {
    plugin: {
      kind: 'console',
      entry: '.',
      tier: 'first-party',
      inject: ['slots'],
      provides: [],
      slots: ['shell.sidebar.server'],
      permissions: [],
      requires: { console: '^0.1.0' },
      editions: ['oss', 'cloud'],
    },
  },
};

export function HostedHeader({
  session,
  watch = watchHealth,
}: {
  session: HostedSession;
  /** The /health poller; tests pass their own. */
  watch?: (origin: string, onStatus: (s: EngineStatus) => void) => () => void;
}) {
  const host = new URL(session.engine).host;
  const [status, setStatus] = useState<EngineStatus>({ state: 'checking' });
  useEffect(() => watch(session.engine, setStatus), [watch, session.engine]);
  const said = engineStatusText(status, session.engine);
  const title = session.changeEngine ? `${said} Change engine` : said;
  const dot = (
    <>
      <span
        className={`lc-hosted-dot lc-hosted-dot-${status.state}`}
        data-engine-status={status.state}
        aria-hidden="true"
      />
      <span>{host}</span>
      <span className="sr-only">{said}</span>
    </>
  );
  return (
    <div className="lc-hosted">
      {session.changeEngine ? (
        <button
          type="button"
          className="lc-hosted-engine"
          title={title}
          onClick={session.changeEngine}
        >
          {dot}
        </button>
      ) : (
        <span className="lc-hosted-engine" title={title}>
          {dot}
        </span>
      )}
      <label
        className="lc-hosted-demo"
        title={`Shows pages that run on built-in sample data, not your engine: ${DEMO_PAGES.join(', ')}.`}
      >
        <input
          type="checkbox"
          checked={session.demo}
          onChange={(e) => session.setDemo(e.currentTarget.checked)}
        />
        <span>Demo data</span>
      </label>
      {session.demo && (
        <span className="lc-hosted-demo-on" role="status">
          Sample pages on
        </span>
      )}
      <HostedIdentity session={session} />
      {session.signOut && (
        <Button size="sm" variant="quiet" onClick={session.signOut}>
          {session.authMode === 'apikey' ? 'Forget key' : 'Sign out'}
        </Button>
      )}
    </div>
  );
}

/** The engine dot's tooltip and screen-reader text. */
export function engineStatusText(status: EngineStatus, engine: string): string {
  if (status.state === 'checking') return `Checking the engine at ${engine}...`;
  if (status.state === 'ok') return `Connected to ${engine}.`;
  const secs = Math.max(1, Math.round(status.retryInMs / 1000));
  const why = /fetch|network|load failed/i.test(status.message)
    ? 'No answer. Check that it is running and that this device is on its tailnet.'
    : status.message;
  return `Cannot reach the engine at ${engine}. ${why} Trying again in ${secs} s.`;
}

/** Who the console is acting as, and how the engine knows. */
function HostedIdentity({ session }: { session: HostedSession }) {
  if (session.authMode === 'apikey')
    return (
      <span className="lc-hosted-user" title="Signed in to the engine with an API key">
        API key
      </span>
    );
  if (session.authMode === 'tailscale' && !session.user)
    return (
      <span
        className="lc-hosted-user lc-hosted-user-unknown"
        title={session.identityProblem ?? 'The engine did not say who you are.'}
      >
        Tailnet user unknown
      </span>
    );
  if (session.authOff)
    return (
      <span
        className="lc-hosted-user"
        title="The engine runs with sign-in off (LOAMS_AUTH_MODE=off): it listens on loopback only and every caller is the local developer."
      >
        {session.user ?? 'Local developer'}
        <span className="lc-hosted-via"> · sign-in off</span>
      </span>
    );
  if (!session.user) return null;
  return (
    <span
      className="lc-hosted-user"
      title={
        session.authMode === 'tailscale'
          ? `${session.user}, identified by the engine from your tailnet login`
          : `Signed in as ${session.user}`
      }
    >
      {session.user}
      {session.authMode === 'tailscale' && <span className="lc-hosted-via"> via tailnet</span>}
    </span>
  );
}

/** The header plugin; `session` comes from the hosted gate, not from a service. */
export function hostedPlugin(session: HostedSession): PluginModule {
  return {
    name: 'console-hosted',
    inject: ['slots'],
    apply(ctx: Context) {
      const slots = service(ctx, 'slots');
      ctx.effect(() =>
        slots.register({ name: 'shell.sidebar.server', plugin: 'console-hosted' }, () => (
          <HostedHeader session={session} />
        )),
      );
    },
  };
}

/** The catalog row that loads the header plugin. */
export const hostedPatch: CatalogPatch = [
  { insert: [{ id: 'console-hosted', name: HOSTED_PACKAGE }] },
];

export const DEMO_PACKAGE = '@loams/console-demo';

export const demoManifest = {
  name: DEMO_PACKAGE,
  version: '0.1.0',
  loams: {
    plugin: {
      kind: 'console',
      entry: '.',
      tier: 'first-party',
      inject: ['slots'],
      provides: [],
      slots: ['shell.sidebar.server'],
      permissions: [],
      requires: { console: '^0.1.0' },
      editions: ['oss', 'cloud'],
    },
  },
};

export const demoPatch: CatalogPatch = [{ insert: [{ id: 'console-demo', name: DEMO_PACKAGE }] }];

/**
 * Demo mode's header, in place of HostedHeader: there is no engine to watch and no
 * one to sign in as, and the visitor has to be able to tell sample data from theirs.
 */
export function DemoHeader() {
  return (
    <div className="lc-hosted">
      <span className="lc-hosted-demo-on" role="status">
        Demo data
      </span>
      <span
        className="lc-hosted-user"
        title="The console's built-in sample data (catalog/samples.yml), served by an in-browser mock of the Loams APIs. Nothing is sent anywhere and nothing is stored."
      >
        Sample data, in this browser
      </span>
      <a className="lc-hosted-user lc-demo-leave" href={demoHref(false)}>
        Connect your own engine
      </a>
    </div>
  );
}

/** The demo header plugin; no session, because there is no engine. */
export function demoPlugin(): PluginModule {
  return {
    name: 'console-demo',
    inject: ['slots'],
    apply(ctx: Context) {
      const slots = service(ctx, 'slots');
      ctx.effect(() =>
        slots.register({ name: 'shell.sidebar.server', plugin: 'console-demo' }, () => (
          <DemoHeader />
        )),
      );
    },
  };
}
