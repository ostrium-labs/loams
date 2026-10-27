// The Seed Exchange end to end on the web edition (plan MK1 Task 9): Studio's harness
// picker links to the marketplace's harnesses, the consent screen shows the plan
// before anything is created, Install installs it, and the harness appears in the
// picker.
//
// The daemon is a debug build with `--features e2e-marketplace` and
// LOAMS_AGENTD_TEST_MARKETPLACE: its marketplace is
// the testkit's world (a signed fake official index with `testkit/echo-acp`, stand-in
// signatures and a fake npm), so nothing leaves this machine.
//
// Opening a thread on the installed harness is not covered: a thread cannot run on a
// marketplace harness yet (Task 4b ruling 8; Task 9 ruling 2). The picker lists it
// without offering it, and this spec checks that.

import { type BrowserContext, expect, type Page, test } from '@playwright/test';
import { type Agentd, startAgentd } from './fixtures/agentd';
import { go, modes, projectHref } from './studio';

const HARNESS = 'testkit/echo-acp';

async function addHost(page: Page, agentd: Agentd) {
  await go(page, '/studio/settings');
  const form = page.getByRole('form', { name: 'Add an agentd host' });
  await form.getByLabel('Name').fill('E2E host');
  await form.getByLabel('URL', { exact: true }).fill(agentd.url ?? '');
  await form.getByRole('button', { name: 'Add host' }).click();
  const row = page.getByRole('list', { name: 'Registered hosts' }).getByRole('listitem', {
    name: 'E2E host',
  });
  await row.getByLabel('Access token for E2E host').fill(agentd.token ?? '');
  await row.getByRole('button', { name: 'Set token' }).click();
  await expect(row).toContainText('signed in');
}

test.describe('the Seed Exchange on the web edition', () => {
  let agentd: Agentd;
  let context: BrowserContext;

  test.beforeAll(async ({ baseURL }) => {
    agentd = await startAgentd({
      mode: 'remote',
      allowOrigin: new URL(baseURL ?? 'http://127.0.0.1:5191').origin,
      env: { LOAMS_AGENTD_TEST_MARKETPLACE: '1' },
    });
    await agentd.call('/loams.agentd.v1.ProjectService/AddRepo', { path: agentd.repo });
  });

  test.afterAll(async () => {
    await context?.close();
    if (agentd) {
      if (test.info().status !== 'passed') console.log(agentd.log().slice(-4000));
      await agentd.stop();
    }
  });

  test('marketplace_install_harness_appears_in_picker', async ({ browser }) => {
    // The testkit marketplace is in a debug daemon built with `--features
    // e2e-marketplace` only (studio-e2e.yml builds it so); any other daemon would reach
    // the network, so the spec is skipped there.
    await expect
      .poll(() => agentd.log().includes('LOAMS_AGENTD_TEST_MARKETPLACE is set'), { timeout: 5_000 })
      .toBe(true)
      .catch(() => undefined);
    test.skip(
      !agentd.log().includes('LOAMS_AGENTD_TEST_MARKETPLACE is set'),
      'loams-agentd was built without --features e2e-marketplace',
    );
    context = await browser.newContext();
    const page = await context.newPage();
    await page.goto('/?demo#/studio');
    await expect(modes(page).studio).toHaveAttribute('aria-pressed', 'true', { timeout: 60_000 });
    await addHost(page, agentd);

    // The picker's "Add a harness…" opens the Seed Exchange filtered to harnesses.
    await go(page, projectHref(agentd.repo));
    const picker = page.getByRole('combobox', { name: 'Harness' });
    await expect(picker).toHaveText(/Mock/);
    await picker.click();
    await page.getByRole('option', { name: 'Add a harness…' }).click();
    await expect
      .poll(() => page.evaluate(() => location.hash))
      .toBe('#/studio/marketplace?kind=harness');
    await expect(page.getByRole('heading', { name: /The Seed Exchange/ })).toBeVisible();
    await expect(page.getByText('Experimental')).toBeVisible();

    // The listing, then the consent screen made from PlanInstall.
    const card = page.getByRole('listitem').filter({ hasText: HARNESS }).getByRole('article');
    await expect(card).toBeVisible({ timeout: 30_000 });
    await card.getByRole('button', { name: 'Details' }).click();
    await page.getByRole('button', { name: 'Install…' }).click();
    const consent = page.getByRole('form', { name: /^Install / });
    await expect(consent).toBeVisible();
    await expect(consent.getByRole('list', { name: 'It can', exact: true })).toContainText(
      'api.example.com',
    );
    await expect(consent).toContainText('Nothing is created until you confirm.');
    // Nothing is created before consent.
    const before = await agentd.call<{ installed?: unknown[] }>(
      '/loams.agentd.v1.MarketplaceService/ListInstalled',
      {},
    );
    expect(before.installed ?? []).toHaveLength(0);

    await consent.getByRole('button', { name: 'Install' }).click();
    await expect(page.getByText(`${HARNESS} is installed`)).toBeVisible({ timeout: 60_000 });
    const after = await agentd.call<{ installed?: { id: string }[] }>(
      '/loams.agentd.v1.MarketplaceService/ListInstalled',
      {},
    );
    expect((after.installed ?? []).map((i) => i.id)).toEqual([HARNESS]);

    const listed = await agentd.call<{ harnesses?: { marketplaceListing?: string }[] }>(
      '/loams.agentd.v1.HarnessService/ListHarnesses',
      {},
    );
    expect((listed.harnesses ?? []).map((h) => h.marketplaceListing)).toContain(HARNESS);
    // The harness is in Studio's picker, listed but not offered until threads can run on it.
    await go(page, projectHref(agentd.repo));
    await page.getByRole('combobox', { name: 'Harness' }).click();
    const option = page.getByRole('option', { name: /threads on it come later/ });
    await expect(option).toBeVisible({ timeout: 30_000 });
    await expect(option).toHaveAttribute('aria-disabled', 'true');
  });
});
