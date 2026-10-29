// @loams/plugin-studio's skeleton (SD1 Task 15, §53 §5.1–§5.2): the manifest, the
// empty states without agentd, adding a project on each edition, and the layout (design 59:
// the page column, its commands sent to the shell's inspector and drawer).

import { validateManifest } from '@loams/console-host';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import type { StudioAgentd } from '../src/agentd.js';
import { StudioLayout } from '../src/layout/StudioLayout.js';
import { StudioCommands } from '../src/palette/commands.js';
import type { StudioRoute } from '../src/routes.js';
import { desktopFake, webFake, wrap } from './fakes.js';
import { StudioInShell, TestPanel } from './shell.js';

/** jsdom has no matchMedia: answer every query with `wide`. */
function viewport(wide: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query.includes('min-width') ? wide : !wide,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
}

function show(agentd: StudioAgentd, route: StudioRoute = { kind: 'home' }) {
  const navigate = vi.fn();
  render(wrap(<StudioInShell agentd={agentd} route={route} navigate={navigate} />));
  return { navigate };
}

beforeEach(() => {
  localStorage.clear();
  viewport(true);
  // Empty's grain canvas observes its size; jsdom has no ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the plugin', () => {
  it('studio_plugin_manifest_valid', () => {
    const m = validateManifest(pkg);
    expect(m.modes).toEqual(['studio']);
    expect(m.editions).toEqual(['oss', 'desktop', 'cloud']);
    expect(m.tier).toBe('first-party');
    expect(m.permissions).toEqual(['agentd:use', 'agentd:git-write', 'ui:notifications']);
    expect(m.inject).toEqual(
      expect.arrayContaining(['router', 'slots', 'mode', 'platform', 'flags', 'agentd', 'desktop']),
    );
  });
});

describe('empty states', () => {
  it('empty_state_without_agentd_desktop', async () => {
    const { agentd, mock } = desktopFake({ running: false });
    show(agentd);
    expect(await screen.findByText('Start the daemon')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Start' }));
    await waitFor(() => expect(mock.calls).toContain('start'));
    // Started: the projects show.
    const sidebar = screen.getByRole('navigation', { name: 'Projects' });
    expect(await within(sidebar).findByRole('link', { name: /^loams/ })).toBeTruthy();
  });

  it('empty_state_without_host_web', async () => {
    show(webFake().agentd);
    expect(await screen.findByText('Studio needs a Loams server with agentd')).toBeTruthy();
    const link = screen.getByRole('link', { name: 'Agentd hosts' });
    expect(link.getAttribute('href')).toBe('#/studio/settings');
  });

  it('no projects, then no threads', async () => {
    show(desktopFake({ repos: [], folder: null }).agentd);
    expect(await screen.findByText('No projects yet')).toBeTruthy();
    cleanup();
    const withOne = desktopFake();
    show(withOne.agentd, { kind: 'project', projectId: '~/src/loams' });
    expect(await screen.findByText('No threads yet')).toBeTruthy();
  });

  it('an unknown project says so', async () => {
    show(desktopFake().agentd, { kind: 'project', projectId: '/nowhere' });
    expect(await screen.findByText('No such project')).toBeTruthy();
  });
});

describe('adding a project', () => {
  it('add_project_desktop_uses_folder_picker', async () => {
    const { agentd, daemon, pickFolder } = desktopFake();
    show(agentd);
    const sidebar = await screen.findByRole('navigation', { name: 'Projects' });
    await within(sidebar).findByRole('link', { name: /loams/ });
    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    await waitFor(() => expect(pickFolder).toHaveBeenCalledOnce());
    // ProjectService.AddRepo over the agentd transport, not the desktop bridge.
    await waitFor(() => expect(daemon.calls).toContain('AddRepo:/home/omar/src/site'));
    expect(await within(sidebar).findByRole('link', { name: /^site/ })).toBeTruthy();
    // No URL dialog on the desktop.
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('a cancelled folder picker adds nothing', async () => {
    const { agentd, daemon, pickFolder } = desktopFake({ folder: null });
    show(agentd);
    await screen.findByRole('navigation', { name: 'Projects' });
    fireEvent.click(await screen.findByRole('button', { name: 'Add project' }));
    await waitFor(() => expect(pickFolder).toHaveBeenCalled());
    expect(daemon.calls.filter((c) => c.startsWith('AddRepo'))).toEqual([]);
  });

  it('add_project_web_clones', async () => {
    const { agentd, daemon } = webFake({
      host: { id: 'h1', name: 'build box' },
      repos: [{ path: '/srv/src/loams', name: 'loams', defaultBranch: 'dev' }],
    });
    const cloned = () => daemon.calls.filter((c) => c.startsWith('CloneRepo:'));
    show(agentd);
    fireEvent.click(await screen.findByRole('button', { name: 'Add project' }));
    const dialog = await screen.findByRole('dialog');
    const input = within(dialog).getByLabelText('Repository URL');
    // Only remote git URLs are offered to the daemon.
    fireEvent.change(input, { target: { value: 'file:///etc' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Clone' }));
    expect(await within(dialog).findByText(/https, ssh or user@host:path/)).toBeTruthy();
    expect(cloned()).toEqual([]);
    fireEvent.change(input, { target: { value: 'https://git.example.com/acme/api.git' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Clone' }));
    await waitFor(() =>
      expect(cloned()).toEqual(['CloneRepo:https://git.example.com/acme/api.git']),
    );
    const sidebar = screen.getByRole('navigation', { name: 'Projects' });
    expect(await within(sidebar).findByRole('link', { name: /api/ })).toBeTruthy();
  });
});

describe('the layout (design 59: the shell owns the sidebar, inspector and drawer)', () => {
  it('is the page column only, and sends its panel and terminal commands to the shell', () => {
    const commands = new StudioCommands();
    const inspector = new TestPanel();
    const drawer = new TestPanel();
    render(
      <StudioLayout commands={commands} inspector={inspector} drawer={drawer}>
        <p>main body</p>
      </StudioLayout>,
    );
    expect(screen.getByText('main body')).toBeTruthy();
    expect(screen.queryByRole('separator')).toBeNull();
    expect(screen.queryByRole('complementary')).toBeNull();

    act(() => void commands.run('rightPanel.toggle'));
    expect(inspector.state().open).toBe(true);
    act(() => void commands.run('rightPanel.toggle'));
    expect(inspector.state().open).toBe(false);
    act(() => void commands.run('rightPanel.show'));
    expect(inspector.state().open).toBe(true);

    act(() => void commands.run('terminal.toggle'));
    expect(drawer.state()).toMatchObject({ open: true, tab: 'studio.terminal' });
    act(() => void commands.run('terminal.toggle'));
    expect(drawer.state().open).toBe(false);
  });
});
