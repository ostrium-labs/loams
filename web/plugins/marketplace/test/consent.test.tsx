// The consent screen (plan MK1 Task 9; design §73 §7.2, D1466): every permission in
// agentd's PlanInstall fixture is on the screen, Install sends exactly the plan's
// digest, and the marketplace errors are said plainly.

import { Code, ConnectError } from '@connectrpc/connect';
import type { InstallPlan } from '@loams/agentd-client';
import { ErrorInfoSchema } from '@loams/proto/errors';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { ConsentScreen, dataScopes } from '../src/consent.js';
import { marketplaceError } from '../src/errors.js';
import { tierLabel } from '../src/labels.js';
import { InstallPage } from '../src/pages.js';
import { fakeApi, PLAN, wrap } from './fake.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

/** A plan with every part the fixture leaves empty: data scopes, roles, models, budgets. */
const FULL: InstallPlan = {
  ...PLAN,
  place: {
    ...PLAN.place,
    data: [
      { kind: 'database', name: 'issues', access: 'write' },
      { kind: 'bucket', name: 'attachments', access: 'read' },
    ],
    roles: ['triage'],
  },
  dependencies: [
    ...PLAN.dependencies,
    { id: 'ostrium/notes-mcp', version: '^1', installed: false },
  ],
  secrets: [
    ...PLAN.secrets,
    { name: 'webhook', description: 'An optional webhook.', required: false },
  ],
  models: {
    requires: { tools: true, vision: true, context_min: 128000 },
    suggested: { provider: 'anthropic', model: 'claude-sonnet' },
    api: ['anthropic-messages'],
  },
  cost: {
    typical_tokens_per_run: 20000,
    max_tokens_per_run: 100000,
    typical_runs_per_day: 12,
    budget_usd_per_run: 0.5,
    budget_usd_per_day: 5,
  },
  versionAgeDays: null,
};

const strings = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((s): s is string => typeof s === 'string') : [];

/** Every string of the plan a person must see before consenting. */
function permissionTexts(plan: InstallPlan): string[] {
  const out: string[] = [
    plan.summary.id,
    plan.summary.title,
    plan.version,
    plan.licence,
    plan.sourceRepository,
    plan.digest,
    tierLabel(plan.summary.tier),
    ...dataScopes(plan.place),
    ...strings(plan.place?.always_ask),
    ...strings(plan.place?.egress),
    ...strings(plan.place?.roles),
    ...plan.egress,
    ...plan.cannot,
  ];
  for (const t of plan.tools) {
    out.push(t.name, t.description);
    if (t.mcpServer) out.push(t.mcpServer);
  }
  for (const d of plan.dependencies) out.push(d.id, d.version);
  for (const s of plan.secrets) out.push(s.name, s.description);
  const provenance = plan.provenance as Record<string, string> | null;
  if (provenance) out.push(provenance.type ?? '', provenance.ref ?? '');
  if (plan.summary.revocation) out.push(plan.summary.revocation.reason);
  return out.filter((s) => s !== '');
}

function show(plan: InstallPlan, extra: Partial<Parameters<typeof ConsentScreen>[0]> = {}) {
  const confirmed: Record<string, string>[] = [];
  let replanned = 0;
  render(
    <ConsentScreen
      plan={plan}
      onConfirm={(s) => confirmed.push(s)}
      onCancel={() => undefined}
      onReplan={() => {
        replanned += 1;
      }}
      {...extra}
    />,
  );
  return { confirmed, replanned: () => replanned };
}

const form = () => screen.getByRole('form');

describe('the consent screen', () => {
  it('shows every permission in the PlanInstall fixture', () => {
    show(PLAN);
    const page = form().textContent ?? '';
    for (const text of permissionTexts(PLAN)) expect(page, text).toContain(text);
    // Tools by risk, the most dangerous first, and the one that always asks says so.
    const can = within(screen.getByRole('list', { name: 'It can' }));
    const rows = can.getAllByRole('listitem');
    expect(rows[0]?.textContent).toContain('Destructive');
    expect(rows[0]?.textContent).toContain('merge_pull_request');
    expect(rows[0]?.textContent).toContain('Asks you every time');
    expect(rows[1]?.textContent).toContain('Reads');
    expect(rows[1]?.textContent).not.toContain('Asks you every time');
    expect(can.getByText('Reach api.github.com')).toBeTruthy();
    // The can't list, the always-ask approvals, the network.
    expect(
      within(screen.getByRole('list', { name: "It can't" })).getByText(
        'Reach any host but api.github.com.',
      ),
    ).toBeTruthy();
    expect(
      within(screen.getByRole('list', { name: 'Asks you every time' })).getByText(
        'merge_pull_request',
      ),
    ).toBeTruthy();
    expect(
      within(screen.getByRole('list', { name: 'Network' })).getByText('api.github.com'),
    ).toBeTruthy();
    // The dependency is reused; the secret is asked for; the version's age and the
    // advisory on it are there; models and budgets say there are none.
    expect(page).toContain('already installed, reused');
    expect(screen.getByLabelText(/token/)).toBeTruthy();
    expect(page).toContain('Published 4 days ago.');
    expect(page).toContain('An upstream advisory affects some tools.');
    expect(page).toContain('No model requirements declared');
    expect(page).toContain('No budget proposed.');
    expect(page).toContain('First-party');
    expect(page).toContain('MCP server');
    expect(page).toContain('Runs on Loams');
    expect(page).toContain('Go');
  });

  it('shows the data scopes, roles, models, budgets and new dependencies of a fuller plan', () => {
    show(FULL);
    const page = form().textContent ?? '';
    for (const text of permissionTexts(FULL)) expect(page, text).toContain(text);
    expect(page).toContain('Read and write the database issues');
    expect(page).toContain('Read the bucket attachments');
    expect(page).toContain('Use the role triage');
    expect(page).toContain('installed with it');
    expect(page).toContain('Needs a model with tool calls.');
    expect(page).toContain('Needs a model with vision.');
    expect(page).toContain('at least 128000 tokens');
    expect(page).toContain("Suggests anthropic's claude-sonnet.");
    expect(page).toContain('Speaks anthropic-messages.');
    expect(page).toContain('About 20000 tokens a run.');
    expect(page).toContain('At most 100000 tokens a run.');
    expect(page).toContain('About 12 runs a day.');
    expect(page).toContain('$0.5 a run');
    expect(page).toContain('$5 a day');
    expect(page).toContain('does not say when this version was published');
    expect(screen.getByLabelText(/webhook/)).toBeTruthy();
  });

  it('asks for the required secrets before Install', () => {
    const { confirmed } = show(PLAN);
    const install = screen.getByRole('button', { name: 'Install' }) as HTMLButtonElement;
    expect(install.disabled).toBe(true);
    expect(form().textContent).toContain('Supply token first.');
    fireEvent.change(screen.getByLabelText(/token/), { target: { value: 'ghp_x' } });
    expect(install.disabled).toBe(false);
    fireEvent.click(install);
    expect(confirmed).toEqual([{ token: 'ghp_x' }]);
  });

  it('refuses a revoked version', () => {
    show({
      ...PLAN,
      summary: {
        ...PLAN.summary,
        revocation: { severity: 'disable', reason: 'Malicious.', versions: '*', advisory: null },
      },
    });
    fireEvent.change(screen.getByLabelText(/token/), { target: { value: 'x' } });
    expect((screen.getByRole('button', { name: 'Install' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(form().textContent).toContain('A revoked version cannot be installed.');
  });

  it('says plan_changed plainly and reads the plan again', () => {
    const { replanned } = show(PLAN, {
      error: marketplaceError(reasoned('plan_changed')),
    });
    expect(screen.getByRole('alert').textContent).toContain(
      'The listing changed after this plan was made',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Read the new plan' }));
    expect(replanned()).toBe(1);
  });
});

function reasoned(reason: string) {
  return new ConnectError(`${reason}: from agentd`, Code.FailedPrecondition, undefined, [
    { desc: ErrorInfoSchema, value: { reason } },
  ]);
}

describe('marketplace errors', () => {
  it('says each marketplace reason plainly and keeps any other message', () => {
    expect(marketplaceError(reasoned('consent_required')).message).toMatch(/needs your consent/);
    expect(marketplaceError(reasoned('listing_revoked')).message).toMatch(/revoked or yanked/);
    expect(marketplaceError(reasoned('untrusted_listing')).message).toMatch(/did not check out/);
    expect(marketplaceError(reasoned('listing_revoked')).reason).toBe('listing_revoked');
    expect(marketplaceError(new Error('npm was not found')).message).toBe('npm was not found');
  });
});

describe('the install page', () => {
  it('installs exactly the plan the person read, by its digest', async () => {
    const { api, calls } = fakeApi();
    const nav: string[] = [];
    render(
      <InstallPage
        api={{ api, key: 1 }}
        base="/studio/marketplace"
        params={{ id: PLAN.summary.id, source: 'official' }}
        navigate={(to) => void nav.push(to)}
      />,
      { wrapper: wrap(api) },
    );
    fireEvent.change(await screen.findByLabelText(/token/), { target: { value: 'ghp_x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Install' }));
    await waitFor(() => expect(nav).toHaveLength(1));
    expect(calls.installs).toEqual([
      {
        id: 'ostrium/github-mcp-server',
        version: '2.0.3',
        source: 'official',
        planDigest: PLAN.digest,
        config: {},
        secrets: { token: 'ghp_x' },
      },
    ]);
    expect(nav[0]).toBe('/studio/marketplace/installed?done=ostrium%2Fgithub-mcp-server');
  });

  it('shows plan_changed, and confirms the new plan only after it is read again', async () => {
    let fail = true;
    const { api, calls } = fakeApi({
      install: () => (fail ? Promise.reject(reasoned('plan_changed')) : undefined),
    });
    render(
      <InstallPage
        api={{ api, key: 2 }}
        base="/marketplace"
        params={{ id: PLAN.summary.id }}
        navigate={() => undefined}
      />,
      { wrapper: wrap(api) },
    );
    fireEvent.change(await screen.findByLabelText(/token/), { target: { value: 'ghp_x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Install' }));
    expect((await screen.findByRole('alert')).textContent).toContain('nothing was installed');
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Read the new plan' }));
    await waitFor(() => expect(calls.plans).toBe(2));
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
  });

  it('sends an update that widens a permission with the new plan digest', async () => {
    const { api, calls } = fakeApi();
    render(
      <InstallPage
        api={{ api, key: 3 }}
        base="/marketplace"
        params={{ id: PLAN.summary.id, version: '2.0.3', update: '1', from: '2.0.2' }}
        navigate={() => undefined}
      />,
      { wrapper: wrap(api) },
    );
    expect(await screen.findByText(/2.0.2 is installed/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText(/token/), { target: { value: 'x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Update' }));
    await waitFor(() =>
      expect(calls.updates).toEqual([
        { id: 'ostrium/github-mcp-server', version: '2.0.3', planDigest: PLAN.digest },
      ]),
    );
    expect(calls.installs).toEqual([]);
  });
});
