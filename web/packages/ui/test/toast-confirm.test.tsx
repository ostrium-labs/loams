import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import {
  ConfirmDialog,
  resolveSidebarState,
  Toaster,
  ToastProvider,
  TypedConfirmDialog,
  toastManager,
  useToast,
} from '../src';

afterEach(cleanup);
beforeEach(() => vi.useRealTimers());
Element.prototype.getAnimations ??= () => [];

function Raise({ onReady }: { onReady: (t: ReturnType<typeof useToast>) => void }) {
  onReady(useToast());
  return null;
}

describe('toasts (D1082)', () => {
  it('stack bottom right, with an action, and the old danger type still works', async () => {
    let toast!: ReturnType<typeof useToast>;
    const undo = vi.fn();
    render(
      <ToastProvider>
        <Raise onReady={(t) => (toast = t)} />
        <Toaster />
      </ToastProvider>,
    );
    act(() => {
      toast.add({ title: 'Branch created', type: 'success' });
      toast.add({ title: 'Delete failed', description: 'locked', type: 'danger' });
      toast.add({ title: 'Table dropped', actionProps: { children: 'Undo', onClick: undo } });
    });
    const viewport = await screen.findByRole('region', { name: /notifications/i });
    expect(viewport.className).toContain('bottom-4');
    expect(viewport.className).toContain('right-4');
    const titles = [...viewport.querySelectorAll('[data-slot="toast-title"]')].map(
      (n) => n.textContent,
    );
    expect(titles).toHaveLength(3);
    expect(titles).toEqual(
      expect.arrayContaining(['Branch created', 'Delete failed', 'Table dropped']),
    );
    expect(viewport.querySelector('[data-type="error"]')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(undo).toHaveBeenCalled();
  });

  it('auto-dismisses after its timeout', async () => {
    vi.useFakeTimers();
    render(
      <ToastProvider>
        <Toaster />
      </ToastProvider>,
    );
    act(() => {
      toastManager.add({ title: 'Saved', timeout: 1000 });
    });
    expect(screen.getByText('Saved')).toBeTruthy();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(screen.queryByText('Saved')).toBeNull();
  });
});

describe('confirmations (D1082)', () => {
  it('a destructive confirmation names the object and its impact', async () => {
    render(
      <ConfirmDialog
        open
        onClose={() => {}}
        onConfirm={() => {}}
        object="the table orders"
        impact="Its 1,204 rows are removed from this branch."
        confirm="Drop table"
      />,
    );
    await act(async () => {});
    const dialog = screen.getByRole('alertdialog', { name: /Drop the table orders\?/ });
    expect(dialog.textContent).toContain('Its 1,204 rows are removed from this branch.');
  });

  it('typed confirmation stays disabled until the phrase is typed exactly', async () => {
    const onConfirm = vi.fn();
    render(
      <TypedConfirmDialog
        open
        onClose={() => {}}
        onConfirm={onConfirm}
        object="the table orders on production"
        impact="Production data is deleted. A branch restore is the only way back."
        phrase="production"
        confirm="Drop table"
      />,
    );
    await act(async () => {});
    const button = screen.getByRole('button', { name: 'Drop table' });
    expect(button.hasAttribute('disabled')).toBe(true);
    const input = screen.getByRole('textbox', { name: /Type production to confirm/ });
    fireEvent.change(input, { target: { value: 'Production' } });
    expect(button.hasAttribute('disabled')).toBe(true);
    fireEvent.change(input, { target: { value: 'production' } });
    expect(button.hasAttribute('disabled')).toBe(false);
    fireEvent.click(button);
    expect(onConfirm).toHaveBeenCalledOnce();
  });
});

describe('sidebarState', () => {
  it('is upstream as-is', () => {
    expect(resolveSidebarState({ isMobile: true, open: true, openMobile: false })).toBe(
      'collapsed',
    );
    expect(resolveSidebarState({ isMobile: false, open: true, openMobile: false })).toBe(
      'expanded',
    );
  });
});
