import type { FlagsService, PlatformService } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import plugin, { StatusPage } from '../src/index.js';

afterEach(cleanup);

const platform = { kind: 'web', baseUrl: 'http://127.0.0.1:8084' } as PlatformService;

function flags(apiVersions: string[], edition: FlagsService['edition'] = 'oss'): FlagsService {
  return {
    edition,
    instanceName: 'Loams (mock)',
    serverVersion: '0.0.1',
    features: {},
    apiVersions,
    has: (api) => apiVersions.includes(api),
  };
}

describe('StatusPage', () => {
  it('shows which app APIs the instance serves', () => {
    render(
      <SlotProvider registry={new SlotRegistry()}>
        <StatusPage
          flags={flags(['loams.instance.v1', 'loams.approvals.v1'])}
          platform={platform}
        />
      </SlotProvider>,
    );
    expect(screen.getByRole('heading', { name: 'Loams (mock)' })).toBeTruthy();
    expect(screen.getAllByText('served')).toHaveLength(2);
    expect(screen.getAllByText('not served')).toHaveLength(3);
  });

  it('says so when the instance does not answer', () => {
    render(
      <SlotProvider registry={new SlotRegistry()}>
        <StatusPage flags={flags([], 'unknown')} platform={platform} />
      </SlotProvider>,
    );
    expect(screen.getByText('This instance did not answer')).toBeTruthy();
  });
});

describe('stack-status plugin', () => {
  const apply = (kind: 'web' | 'desktop') => {
    const slots = new SlotRegistry();
    const pages: string[] = [];
    const ctx = {
      effect: (fn: () => unknown) => void fn(),
      flags: flags(['loams.instance.v1']),
      platform: { kind, baseUrl: 'x' },
      slots,
      router: { page: (spec: { path: string }) => void pages.push(spec.path) },
    };
    plugin.apply(ctx as never, {});
    return { slots, pages };
  };

  it('web_owns_the_home_route', () => {
    const { slots, pages } = apply('web');
    expect(pages).toEqual(['/']);
    expect(slots.entries('environment.overview.card')).toHaveLength(0);
  });

  it('desktop_gives_the_home_route_to_overview_and_keeps_the_api_card', () => {
    const { slots, pages } = apply('desktop');
    expect(pages).toEqual([]);
    expect(slots.entries('environment.overview.card')).toHaveLength(1);
  });
});
