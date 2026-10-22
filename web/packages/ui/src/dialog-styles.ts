// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/dialog-styles.ts at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the design 59 tokens and cx, with Base UI underneath; no cva, lucide or Effect; the glass is the Loams .loams-dialog-glass class; the media classes are dropped (no lightbox); the layers sit above the console's mobile sidebar (60); the enter and exit timing tokens (UI1). Loams' changes are Apache-2.0.

/** The scrim (70) and the viewport (71) sit above the mobile sidebar (60) and below pickers (75). */
const DIALOG_BACKDROP_BASE_CLASS =
  'fixed inset-0 z-[70] transition-opacity duration-(--op-duration-enter) ease-out data-ending-style:duration-(--op-duration-exit) data-ending-style:ease-in data-ending-style:opacity-0 data-starting-style:opacity-0 motion-reduce:transition-none';

export const DIALOG_BACKDROP_CLASS = `loams-dialog-scrim loams-dialog-backdrop ${DIALOG_BACKDROP_BASE_CLASS}`;

const DIALOG_POPUP_BASE_CLASS =
  '-translate-y-[calc(1.25rem*var(--nested-dialogs))] relative flex min-h-0 w-full min-w-0 scale-[calc(1-0.1*var(--nested-dialogs))] flex-col ' +
  'opacity-[calc(1-0.1*var(--nested-dialogs))] text-ink outline-none transition-[scale,opacity,translate] duration-(--op-duration-enter) ease-out data-ending-style:duration-(--op-duration-exit) data-ending-style:ease-in ' +
  'data-nested:data-ending-style:translate-y-8 data-nested:data-starting-style:translate-y-8 data-nested-dialog-open:origin-top ' +
  'data-ending-style:scale-98 data-starting-style:scale-97 data-ending-style:opacity-0 data-starting-style:opacity-0 ' +
  'motion-reduce:transition-none [-webkit-app-region:no-drag]';

export const DIALOG_POPUP_CLASS = `loams-dialog-glass ${DIALOG_POPUP_BASE_CLASS} rounded-xl`;

/** The centring grid every dialog popup sits in. */
export const DIALOG_VIEWPORT_CLASS =
  'fixed inset-0 z-[71] grid grid-rows-[1fr_auto_1fr] justify-items-center p-4';

export const DIALOG_MOBILE_SHEET_CLASS =
  'max-sm:max-w-none max-sm:rounded-none max-sm:border-x-0 max-sm:border-b-0 max-sm:data-ending-style:translate-y-4 max-sm:data-starting-style:translate-y-4';
