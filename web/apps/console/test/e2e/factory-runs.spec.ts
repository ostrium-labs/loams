// The factory's Runs page (plan FT1, FT0 Task 9) in the desktop edition's browser
// preview (`?desktop`, the dev server's fake desktop bridge): the run record is read
// from the Live app `factory` (`runs:list`), mocked here at the network edge, and the
// page shows a run and follows it when the record changes. No daemon is needed.

import { create, toBinary } from '@bufbuild/protobuf';
import { live } from '@loams/proto';
import { expect, test } from '@playwright/test';

type Init = Parameters<typeof create<typeof live.QueryResponseSchema>>[1];

const str = (value: string) => ({ kind: { case: 'stringValue' as const, value } });
const int = (value: number) => ({ kind: { case: 'int64Value' as const, value: BigInt(value) } });

/** `runs:list`'s answer: one run at `stage`. */
function runsAt(stage: string, seq: number): Init {
  return {
    ts: 1n,
    result: {
      kind: {
        case: 'arrayValue',
        value: {
          values: [
            {
              kind: {
                case: 'objectValue',
                value: {
                  fields: {
                    run: str('run-e2e'),
                    seq: int(seq),
                    intent: str('Rate limit signup'),
                    stage: str(stage),
                    state: str(stage === 'close' ? 'succeeded' : 'open'),
                    createdAt: int(1_760_000_000_000),
                    updatedAt: int(1_760_000_000_000 + seq),
                  },
                },
              },
            },
          ],
        },
      },
    },
  };
}

test('factory_runs_page_shows_and_follows_a_run', async ({ page }) => {
  let answer = runsAt('plan', 2);
  const apps: string[] = [];
  await page.route('**/loams.live.v1.LiveService/Query', async (route) => {
    apps.push(route.request().headers()['loams-live-app'] ?? '');
    const body = toBinary(live.QueryResponseSchema, create(live.QueryResponseSchema, answer));
    await route.fulfill({
      status: 200,
      contentType: 'application/proto',
      body: Buffer.from(body),
    });
  });
  await page.goto('/?desktop#/studio/factory/runs');
  const detail = page.getByRole('region', { name: 'Run run-e2e' });
  await expect(detail).toBeVisible({ timeout: 60_000 });
  await expect(detail.locator('[aria-current="step"]')).toHaveText('plan');
  expect(apps).toContain('factory');

  answer = runsAt('review', 5);
  await expect(detail.locator('[aria-current="step"]')).toHaveText('review');
  answer = runsAt('close', 9);
  await expect(detail.locator('[aria-current="step"]')).toHaveText('close');
  await expect(detail.getByText('succeeded')).toBeVisible();
});
