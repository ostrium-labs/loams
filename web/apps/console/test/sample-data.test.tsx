// Every page that renders demonstration data says so (the shared notice from @loams/ui), so no
// visitor mistakes a fixture for their own. Each plugin below is applied with a stub context,
// exactly as the console does, and the page it registers is rendered over the fixture client it
// ships with.

import billing from '@loams/plugin-billing';
import canvas from '@loams/plugin-canvas';
import connectors from '@loams/plugin-connectors';
import deployments from '@loams/plugin-deployments';
import git from '@loams/plugin-git';
import jobs from '@loams/plugin-jobs';
import metrics from '@loams/plugin-metrics';
import pipelines from '@loams/plugin-pipelines';
import { ControlPlane, createMockPgControl } from '@loams/plugin-postgres';
import sql from '@loams/plugin-sql';
import { SAMPLE_DATA_TEXT } from '@loams/ui';
import { cleanup, render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it } from 'vite-plus/test';

// jsdom lacks these; the pages' canvases and dialogs expect them.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

afterEach(cleanup);

type Page = (arg: { params: Record<string, string> }) => ReactNode;
type Plugin = { apply(ctx: never, opts: unknown): void };

/** Applies `plugin` with a stub context and returns the pages it registered, by path. */
function pagesOf(plugin: Plugin): Map<string, Page> {
  const pages = new Map<string, Page>();
  const ctx = {
    effect: (fn: () => unknown) => void fn(),
    router: {
      page: (spec: { path: string }, render: Page) => void pages.set(spec.path, render),
      navigate: () => {},
    },
    slots: { register: () => () => {} },
    platform: { clipboardWrite: async () => {}, fetch: async () => new Response('{}') },
    desktop: {},
  };
  plugin.apply(ctx as never, {});
  return pages;
}

// The fixture-backed pages, by the route that opens them.
const PAGES: [string, Plugin, string][] = [
  ['canvas', canvas as Plugin, '/canvas'],
  ['connectors (instances)', connectors as Plugin, '/connectors/instances'],
  ['jobs', jobs as Plugin, '/jobs'],
  ['sql', sql as Plugin, '/sql'],
  ['metrics', metrics as Plugin, '/metrics'],
  ['pipelines', pipelines as Plugin, '/pipelines'],
  ['git', git as Plugin, '/git'],
  ['deployments', deployments as Plugin, '/deployments'],
  ['billing', billing as Plugin, '/settings/billing'],
];

describe('sample data notice', () => {
  for (const [name, plugin, path] of PAGES) {
    it(`${name} says its data is a sample`, async () => {
      const page = pagesOf(plugin).get(path);
      expect(page, `${name} registers ${path}`).toBeDefined();
      const { container } = render(<>{page?.({ params: {} })}</>);
      await screen.findAllByText(/Sample data/, {}, { timeout: 4000 });
      const notice = container.querySelector('[data-sample-data]');
      expect(notice, `${name} renders the shared notice`).not.toBeNull();
    });
  }

  it('the postgres control plane tab says its data is a sample', async () => {
    const { container } = render(<ControlPlane client={createMockPgControl()} copy={() => {}} />);
    await screen.findByText(/Sample data/);
    expect(container.querySelector('[data-sample-data]')).not.toBeNull();
  });

  it('uses the one sentence everywhere', async () => {
    const page = pagesOf(jobs as Plugin).get('/jobs');
    const { container } = render(<>{page?.({ params: {} })}</>);
    await screen.findByText(/Sample data/);
    expect(container.querySelector('.loams-sample-data')?.textContent).toContain(
      `Sample data — ${SAMPLE_DATA_TEXT}`,
    );
  });
});
