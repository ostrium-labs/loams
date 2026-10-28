import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Toaster, ToastProvider } from '@loams/ui';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { UpdateToast } from '../src/update-toast.js';

afterEach(cleanup);
Element.prototype.getAnimations ??= () => [];

function fakeUpdate() {
  let push: (s: unknown) => void = () => {};
  const update = {
    state: vi.fn(async () => ({ phase: 'idle' as const })),
    onState: (cb: (s: unknown) => void) => {
      push = cb;
      return () => {};
    },
    download: vi.fn(async () => {}),
    installAndRestart: vi.fn(async () => {}),
    skip: vi.fn(async () => {}),
    check: vi.fn(async () => {}),
    autoCheck: vi.fn(async () => ({ enabled: true, locked: false })),
    setAutoCheck: vi.fn(async () => {}),
  } as unknown as LoamsDesktopApi['update'];
  return { update, push: (s: unknown) => act(() => push(s)) };
}

function show(update: LoamsDesktopApi['update']) {
  render(
    <ToastProvider>
      <UpdateToast update={update} />
      <Toaster />
    </ToastProvider>,
  );
}

describe('the update toast (D1085)', () => {
  it('asks before downloading, offers to skip, then to restart', async () => {
    const { update, push } = fakeUpdate();
    show(update);
    push({ phase: 'available', version: '0.2.0', mode: 'self' });
    expect(await screen.findByText('Loams Desktop 0.2.0 is available')).toBeTruthy();
    expect(update.download).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Download' }));
    expect(update.download).toHaveBeenCalledOnce();
    push({ phase: 'downloading', version: '0.2.0', percent: 42.4 });
    expect(await screen.findByText('Downloading 0.2.0')).toBeTruthy();
    expect(screen.getByText('42%')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Download' })).toBeNull();
    push({ phase: 'ready', version: '0.2.0' });
    fireEvent.click(await screen.findByRole('button', { name: 'Restart to update' }));
    expect(update.installAndRestart).toHaveBeenCalledOnce();
  });

  it('skips a version', async () => {
    const { update, push } = fakeUpdate();
    show(update);
    push({ phase: 'available', version: '0.3.0', mode: 'manual' });
    expect(await screen.findByRole('button', { name: 'Download v0.3.0' })).toBeTruthy();
    fireEvent.click(await screen.findByRole('button', { name: 'Skip this version' }));
    expect(update.skip).toHaveBeenCalledWith('0.3.0');
  });

  it('a failed automatic check stays quiet; a failed download says so (T16-2)', async () => {
    const { update, push } = fakeUpdate();
    show(update);
    push({ phase: 'checking' });
    push({ phase: 'error', message: 'feed_unreachable' });
    expect(screen.queryByText('The update failed')).toBeNull();
    push({ phase: 'available', version: '0.2.0', mode: 'self' });
    push({ phase: 'downloading', version: '0.2.0', percent: 10 });
    push({ phase: 'error', message: 'download_hash_mismatch' });
    expect(await screen.findByText('The update failed')).toBeTruthy();
  });

  it('a dismissed offer does not come back for the same version (T16-2)', async () => {
    const { update, push } = fakeUpdate();
    show(update);
    push({ phase: 'available', version: '0.2.0', mode: 'self' });
    await screen.findByText('Loams Desktop 0.2.0 is available');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    await waitFor(() => expect(screen.queryByText('Loams Desktop 0.2.0 is available')).toBeNull());
    push({ phase: 'checking' });
    push({ phase: 'available', version: '0.2.0', mode: 'self' });
    await act(async () => {});
    expect(screen.queryByText('Loams Desktop 0.2.0 is available')).toBeNull();
    push({ phase: 'available', version: '0.2.1', mode: 'self' });
    expect(await screen.findByText('Loams Desktop 0.2.1 is available')).toBeTruthy();
  });
});
