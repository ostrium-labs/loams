// How Studio shows a dev server's preview (design §53 §5.8, §7; SD1 Task 26).
//
//   desktop  the loopback URL (`http://<name>.<project>.localhost:7331`) in a
//            WebContentsView that Electron main places over the tab (D678): its own
//            partition, no preload, sandboxed. `desktop.preview` is the bridge.
//   web      through the remote agentd's `/preview/…` (Q806): the console asks the
//            listener for a short-lived preview ticket with its bearer token, then
//            frames `/preview/pt_<ticket>/` in a sandboxed iframe. A frame cannot send
//            the token, and the listener sets no cookies, so the ticket is the frame's
//            capability: one preview, no longer than the token.
//   neither  a daemon that does not serve `/preview/` (before Task 26): the tab shows
//            the `ssh -L` command that forwards the preview proxy's port.

import type { TokenGetter } from '@loams/agentd-client/transport-remote';
import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import type { PreviewReach, RemotePreview } from '@loams/studio-ui/preview';

/** What the desktop bridge offers (`LoamsDesktopApi.preview`, SD1 Task 26). */
export type DesktopPreviewApi = NonNullable<LoamsDesktopApi['preview']>;

export type { PreviewViewState as DesktopPreviewState } from '@loams/desktop/contracts';

/** Studio's preview host: how to reach previews, and the desktop's view if any. */
export interface StudioPreview {
  reach(): PreviewReach;
  /** Desktop: main's WebContentsView. Absent: an older desktop, so "Open in browser". */
  view?: DesktopPreviewApi;
}

/** The daemon does not serve previews through its remote listener (before Task 26). */
export class PreviewUnsupported extends Error {
  constructor(message = 'This agentd does not serve previews to the browser.') {
    super(message);
    this.name = 'PreviewUnsupported';
  }
}

export function desktopPreview(view: DesktopPreviewApi | undefined): StudioPreview {
  return { reach: () => ({ kind: 'loopback' }), ...(view ? { view } : {}) };
}

const TICKET_BASE = /^\/preview\/pt_[0-9a-f]{64}\/$/;

/**
 * `url` without its trailing slashes, in one pass from the end. `/\/+$/` is quadratic
 * on a long run of `/` that does not end the string (CodeQL js/polynomial-redos).
 */
export function trimTrailingSlashes(url: string): string {
  let end = url.length;
  while (end > 0 && url.charCodeAt(end - 1) === 0x2f) end -= 1;
  return url.slice(0, end);
}

/**
 * Asks the remote agentd at `baseUrl` for a preview ticket for `previewName`
 * (`<name>.<project>`). The request carries the bearer token, never cookies, a
 * referrer or a redirect; the answer must be a ticket path, nothing else.
 */
export function remotePreviewOpener(o: {
  baseUrl: string;
  getToken: TokenGetter;
  fetch?: typeof fetch;
}): (previewName: string) => Promise<RemotePreview> {
  const doFetch = o.fetch ?? ((...args: Parameters<typeof fetch>) => fetch(...args));
  const root = trimTrailingSlashes(o.baseUrl);
  return async (previewName) => {
    const token = await o.getToken();
    if (!token) throw new Error('Add a token for this agentd host in Settings first.');
    const response = await doFetch(`${root}/preview/-/ticket`, {
      method: 'POST',
      headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
      body: JSON.stringify({ preview: previewName }),
      credentials: 'omit',
      referrerPolicy: 'no-referrer',
      redirect: 'error',
      cache: 'no-store',
    });
    if (response.status === 404 || response.status === 501 || response.status === 503) {
      throw new PreviewUnsupported();
    }
    const body = (await response.json().catch(() => null)) as {
      base?: unknown;
      expiresAtMs?: unknown;
      message?: unknown;
    } | null;
    if (!response.ok) {
      throw new Error(
        typeof body?.message === 'string'
          ? body.message
          : `The agentd refused (${response.status}).`,
      );
    }
    if (typeof body?.base !== 'string' || !TICKET_BASE.test(body.base)) {
      throw new Error('The agentd answered an unexpected preview address.');
    }
    return {
      url: `${root}${body.base}`,
      expiresAtMs: typeof body.expiresAtMs === 'number' ? body.expiresAtMs : 0,
    };
  };
}

/** The web's preview host for a remote agentd. */
export function remotePreview(
  open: (previewName: string) => Promise<RemotePreview>,
): StudioPreview {
  return { reach: () => ({ kind: 'remote', open }) };
}
