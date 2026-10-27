// Browse, a listing, installed listings and sources (plan MK1 Task 9).

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { BrowsePage, InstalledPage, ListingPage, SourcesPage } from '../src/pages.js';
import { fakeApi, wrap } from './fake.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

function page(
  View: typeof BrowsePage,
  params: Record<string, string> = {},
  base = '/studio/marketplace',
) {
  const { api, calls } = fakeApi();
  const nav: string[] = [];
  render(
    <View api={{ api, key: 1 }} base={base} params={params} navigate={(to) => void nav.push(to)} />,
    { wrapper: wrap(api) },
  );
  return { calls, nav };
}

describe('browse', () => {
  it('searches with the filters in the route, as the harness picker links', async () => {
    const { calls, nav } = page(BrowsePage, { kind: 'harness' });
    const cards = await screen.findAllByRole('article');
    expect(cards).toHaveLength(2);
    expect(calls.search[0]).toEqual({
      query: '',
      kinds: ['harness'],
      frameworks: [],
      languages: [],
      minLevel: null,
      tiers: [],
      source: null,
    });
    const github = within(screen.getByRole('article', { name: 'GitHub' }));
    expect(github.getByText('First-party')).toBeTruthy();
    expect(github.getByText('Runs on Loams')).toBeTruthy();
    expect(github.getByText(/An upstream advisory affects some tools/)).toBeTruthy();
    await waitFor(() => expect(github.getByText('Installed')).toBeTruthy());
    const starter = within(screen.getByRole('article', { name: 'Loams starter' }));
    expect(starter.getByText('Mastra')).toBeTruthy();
    expect(starter.getByText('TypeScript')).toBeTruthy();

    fireEvent.change(screen.getByLabelText('Tier'), { target: { value: 'firstParty' } });
    expect(nav.at(-1)).toBe('/studio/marketplace?kind=harness&tier=firstParty');
    fireEvent.click(starter.getByRole('button', { name: 'Details' }));
    expect(nav.at(-1)).toBe(
      '/studio/marketplace/listing?id=ostrium%2Floams-starter&source=official',
    );
  });
});

describe('a listing', () => {
  it('reads in plain words and goes to the consent screen', async () => {
    const { nav } = page(
      ListingPage,
      { id: 'ostrium/claude-code', source: 'official' },
      '/marketplace',
    );
    await screen.findByRole('button', { name: /Install/ });
    expect(screen.getByText('Ostrium Labs')).toBeTruthy();
    expect(screen.getByText('Apache-2.0')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /Install/ }));
    expect(nav).toEqual(['/marketplace/install?id=ostrium%2Fclaude-code&source=official']);
  });
});

describe('installed listings', () => {
  it('shows revocation banners and sends a widening update to consent', async () => {
    const { calls, nav } = page(InstalledPage);
    const row = within(await screen.findByRole('listitem', { name: 'ostrium/github-mcp-server' }));
    expect(row.getByText(/An upstream advisory affects some tools/)).toBeTruthy();
    const review = await row.findByRole('button', { name: 'Review update to 2.1.0…' });
    expect(row.getByText(/asks for more than 2.0.3/)).toBeTruthy();
    fireEvent.click(review);
    expect(nav).toEqual([
      '/studio/marketplace/install?id=ostrium%2Fgithub-mcp-server&source=official&version=2.1.0&update=1&from=2.0.3',
    ]);
    fireEvent.click(row.getByRole('button', { name: 'Uninstall' }));
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain("The agent's data stays");
    fireEvent.click(within(dialog).getByRole('button', { name: 'Uninstall' }));
    await waitFor(() => expect(calls.uninstalls).toEqual(['ostrium/github-mcp-server']));
  });
});

describe('sources', () => {
  it('lists the official source and adds another', async () => {
    const { calls } = page(SourcesPage);
    const official = within(await screen.findByRole('listitem', { name: 'official' }));
    expect(official.getByText('The Seed Exchange')).toBeTruthy();
    expect(official.queryByRole('button', { name: 'Remove' })).toBeNull();
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'team' } });
    fireEvent.change(screen.getByLabelText('Index URL'), {
      target: { value: 'https://example.org/index.json' },
    });
    fireEvent.change(screen.getByLabelText('Signing identity'), {
      target: {
        value: 'https://github.com/team/index/.github/workflows/publish.yml@refs/heads/main',
      },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add source' }));
    await waitFor(() => expect(calls.added).toEqual(['team']));
  });
});
