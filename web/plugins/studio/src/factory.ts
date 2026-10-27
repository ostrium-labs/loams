// What Studio reads of the software factory (SD1 Task 27; §53 §5.9, D916, D922).
//
//   desktop: `desktop.factory`, the bridge @loams/plugin-factory also uses. Each app
//            opens on the factory's own pages under `/studio/factory`.
//   web:     nothing until SF1 (`loams.collab.v1` and the `embed` plugin): the
//            Factory group is absent. A `links` source, when one exists, shows the
//            configured apps as plain links to their own UIs (D922): no panels and
//            no embeds.
//
// Studio does not import the factory plugin (§37 §5.4): it reads the same bridge
// and links to the factory's routes.

import type { FactoryHealth, LoamsDesktopApi } from '@loams/desktop/contracts';

export type FactoryState = FactoryHealth;

/** One factory app as the sidebar shows it. */
export interface FactoryApp {
  id: string;
  label: string;
  state: FactoryState;
  /** The app's own UI. */
  url?: string;
  /** Desktop: whether the factory has native panels for it (else its full UI only). */
  hasPanels?: boolean;
  /** The app's preview label, when it is a preview (SigNoz on the House, D1666). */
  preview?: string;
}

/** A pull request as `forgejo.issues` (`type: "pulls"`) returns it. */
export interface FactoryPull {
  id?: string;
  title?: string;
  url?: string;
  head?: string;
}

export interface StudioFactory {
  /** `pages`: the factory's pages are here; `links`: the apps' own UIs only (D922). */
  readonly kind: 'pages' | 'links';
  /** Its reads need a reachable agentd host (the browser): no group without one. */
  readonly needsAgentd?: boolean;
  list(): Promise<FactoryApp[]>;
  /** The open pull requests of a Forgejo repository (desktop). */
  pulls?(owner: string, repo: string): Promise<FactoryPull[]>;
}

export const STATE_LABEL: Record<FactoryState, string> = {
  ok: 'Connected',
  auth_failed: 'Auth failed',
  unreachable: 'Unreachable',
  unconfigured: 'Not configured',
  locked: 'Locked',
};

/** Credentials stored and readable: the app can be opened. */
export const isConfigured = (state: FactoryState) => state !== 'unconfigured' && state !== 'locked';

/** An http(s) URL, or undefined (§53 D919). */
export function httpUrl(url: string | undefined): string | undefined {
  if (!url) return undefined;
  try {
    const u = new URL(url);
    return u.protocol === 'https:' || u.protocol === 'http:' ? url : undefined;
  } catch {
    return undefined;
  }
}

/** The factory's routes in Studio (owned by @loams/plugin-factory). */
export const FACTORY_HOME = '/studio/factory';

/** Where an app's sidebar entry goes: its panels, its full UI, or its configure form. */
export function factoryHref(app: FactoryApp): string {
  if (!isConfigured(app.state)) return `${FACTORY_HOME}/${app.id}/configure`;
  return app.hasPanels === false ? `${FACTORY_HOME}/${app.id}/app` : `${FACTORY_HOME}/${app.id}`;
}

/**
 * The factory through `desktop.factory`, or in a browser through the agentd host's
 * bridge (`studio.factory`, SF1 amendment F1), which has the same reads.
 */
export function desktopFactory(
  desktop: { factory: Pick<LoamsDesktopApi['factory'], 'list' | 'query'> },
  options: { needsAgentd?: boolean } = {},
): StudioFactory {
  return {
    kind: 'pages',
    ...(options.needsAgentd ? { needsAgentd: true } : {}),
    list: async () =>
      (await desktop.factory.list()).map((a) => ({
        id: a.id,
        label: a.label,
        state: a.health,
        url: a.url,
        hasPanels: a.hasPanels,
        ...(a.preview ? { preview: a.preview } : {}),
      })),
    pulls: async (owner, repo) => {
      const r = await desktop.factory.query<FactoryPull[]>({
        app: 'forgejo',
        op: 'issues',
        params: { type: 'pulls', owner, repo, limit: 50 },
      });
      if (!r.ok) throw new Error(r.message);
      return r.value;
    },
  };
}
