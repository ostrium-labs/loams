// The update toast (design 59 D1085; CF1 Task 16), after dsh-desktop's consent-before-download
// flow: an available version is offered, downloads only when the person presses Download,
// then offers a restart; "Skip this version" keeps automatic checks quiet about it. It
// follows `update.onState`, which Electron main pushes on every change. Settings › Updates
// stays the place for "Check now" and the automatic-check switch.

import type { LoamsDesktopApi, UpdateState } from '@loams/desktop/contracts';
import { type ToastOptions, useToast } from '@loams/ui';
import { useEffect, useRef } from 'react';

/** One toast for the updater: each state replaces the last in place. */
export const UPDATE_TOAST_ID = 'loams-update';

/** The toast a state shows, or null for none. `prev` is the phase before this one. */
export function updateToast(
  s: UpdateState,
  prev: UpdateState['phase'] | undefined,
  update: Pick<LoamsDesktopApi['update'], 'download' | 'installAndRestart' | 'skip'>,
): ToastOptions | null {
  const version = s.version ?? '';
  switch (s.phase) {
    case 'available':
      return {
        title: `Loams Desktop ${version} is available`,
        description:
          s.mode === 'manual'
            ? 'It downloads from the release page.'
            : 'Download it now; you choose when to restart.',
        type: 'info',
        timeout: 0,
        actionProps: {
          children: s.mode === 'manual' ? `Download v${version}` : 'Download',
          onClick: () => void update.download(),
        },
        data: {
          actionLayout: 'stacked-end',
          secondaryActionProps: {
            children: 'Skip this version',
            onClick: () => void update.skip(version),
          },
        },
      };
    case 'downloading':
      return {
        title: `Downloading ${version}`,
        description: `${Math.round(s.percent ?? 0)}%`,
        type: 'loading',
        timeout: 0,
      };
    case 'ready':
      return {
        title: `Loams Desktop ${version} is ready`,
        description: 'Restart to finish updating. The local engine stops first.',
        type: 'success',
        timeout: 0,
        actionProps: {
          children: 'Restart to update',
          onClick: () => void update.installAndRestart(),
        },
        data: { actionLayout: 'stacked-end' },
      };
    case 'error':
      // A failed download or install is news; a failed automatic check (offline, say) is not,
      // and Settings › Updates shows it (T16-2).
      if (prev !== 'downloading' && prev !== 'ready') return null;
      return {
        title: 'The update failed',
        description: 'Try again from Settings › Updates.',
        type: 'error',
        timeout: 8000,
      };
    default:
      return null;
  }
}

export function UpdateToast({ update }: { update: LoamsDesktopApi['update'] }) {
  const toast = useToast();
  const prev = useRef<UpdateState['phase'] | undefined>(undefined);
  // The person closed the toast for this phase and version: a later automatic check that
  // finds the same version does not bring it back (T16-2).
  const dismissed = useRef<string | null>(null);
  const shown = useRef(false);

  useEffect(() => {
    let live = true;
    const onState = (s: UpdateState) => {
      if (!live) return;
      const before = prev.current;
      prev.current = s.phase;
      // A periodic check runs while an offer is shown; the offer stays until the answer.
      if (s.phase === 'checking') return;
      const key = `${s.phase}:${s.version ?? ''}`;
      const o = updateToast(s, before, update);
      if (!o) {
        if (shown.current) toast.close(UPDATE_TOAST_ID);
        shown.current = false;
        return;
      }
      if (dismissed.current === key) return;
      dismissed.current = null;
      shown.current = true;
      // Adding with the same id updates the toast in place, merging fields: a state without
      // an action clears the last one's.
      toast.add({
        actionProps: undefined,
        ...o,
        id: UPDATE_TOAST_ID,
        data: {
          ...o.data,
          onClose: () => {
            dismissed.current = key;
            shown.current = false;
          },
        },
      });
    };
    const off = update.onState(onState);
    void update.state().then((s) => {
      // A push that came first is newer than this answer.
      if (prev.current === undefined) onState(s);
    });
    return () => {
      live = false;
      off();
    };
  }, [toast, update]);

  return null;
}
