// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/toast.tsx at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the design 59 tokens and cx; thread routing, the composer store, MorphIcon and copy-to-clipboard removed; the elevated surface and enter and exit timing (UI1). Loams' changes are Apache-2.0.

// Toasts (design 59, D1082): stacked at the bottom right, auto-dismissing, with actions.
// The stack is upstream's: the newest toast in front, older ones peeking behind it,
// spread out while the pointer or focus is on the stack. `ToastProvider`, `Toaster` and
// `useToast` keep the names and call shape they had before CF1 (`type: 'danger'` is the
// error type). Dropped from upstream: the position prop (always bottom right), the
// thread-scoped filter, the anchored toasts, the copy-error button and the
// `additionalActions` and `expandableDescriptionTrigger` data. `buildVisibleToastLayout`
// and `hasVisibleToastAction` are upstream's `toast.logic.ts`, inlined.

import { Toast } from '@base-ui/react/toast';
import {
  type ComponentPropsWithoutRef,
  type CSSProperties,
  type ReactNode,
  useEffect,
  useMemo,
  useState,
} from 'react';
import { Button, type ButtonVariant, buttonClass } from './button';
import { cx } from './cx';
import { Spinner } from './spinner';

/** What a toast carries besides Base UI's own fields. */
export type ToastData = {
  /** Replaces the type's icon. */
  leadingIcon?: ReactNode;
  /** A second, quieter button beside the action. */
  secondaryActionProps?: ComponentPropsWithoutRef<'button'>;
  secondaryActionVariant?: ButtonVariant;
  /** Details hidden behind "Show details". */
  expandableContent?: ReactNode;
  expandableLabels?: { expand?: string; collapse?: string };
  /** `stacked-end` puts the actions on their own row, at the end. */
  actionLayout?: 'inline' | 'stacked-end';
  actionVariant?: ButtonVariant;
  /** Dismiss after this long on screen, counting only while the window is visible and focused. */
  dismissAfterVisibleMs?: number;
  /** Called when the person dismisses the toast. */
  onClose?: () => void;
};

export type ToastType = 'success' | 'error' | 'warning' | 'info' | 'loading' | 'danger';

export type ToastOptions = {
  /** Adding a toast with an id that is already shown updates it in place. */
  id?: string;
  title: ReactNode;
  description?: ReactNode;
  /** `danger` is the error type, kept for callers from before CF1. */
  type?: ToastType;
  /** Milliseconds before it dismisses itself; 0 keeps it until dismissed. Default 5000. */
  timeout?: number;
  priority?: 'low' | 'high';
  /** The action button; its `children` are the label. */
  actionProps?: ComponentPropsWithoutRef<'button'>;
  data?: ToastData;
};

/** The one toast manager. Code outside React (the desktop update toast) adds through it. */
export const toastManager = Toast.createToastManager<ToastData>();
type ToastId = ReturnType<typeof toastManager.add>;
const visibleTimeoutRemainingMs = new Map<ToastId, number>();

/** The old `type: 'danger'` is the error type. */
function normalise<T extends { type?: string }>(o: T): T {
  return o.type === 'danger' ? { ...o, type: 'error' } : o;
}

function typeOf(type: string | undefined): string | undefined {
  return type === 'danger' ? 'error' : type;
}

/* Icons: 16×16, one path each, in the current colour. */

function Glyph({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className={cx('size-4', className)}
    >
      {children}
    </svg>
  );
}

function CircleCheck({ className }: { className?: string }) {
  return (
    <Glyph className={className}>
      <path d="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM8.5 12l2.5 2.5 4.5-5" />
    </Glyph>
  );
}

function CircleAlert({ className }: { className?: string }) {
  return (
    <Glyph className={className}>
      <path d="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM12 8v4.5M12 16h.01" />
    </Glyph>
  );
}

function TriangleAlert({ className }: { className?: string }) {
  return (
    <Glyph className={className}>
      <path d="M10.3 4.2 2.6 17.5A2 2 0 0 0 4.3 20.5h15.4a2 2 0 0 0 1.7-3L13.7 4.2a2 2 0 0 0-3.4 0ZM12 9.5v4M12 17h.01" />
    </Glyph>
  );
}

function Info({ className }: { className?: string }) {
  return (
    <Glyph className={className}>
      <path d="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM12 16v-4.5M12 8h.01" />
    </Glyph>
  );
}

function Loading({ className }: { className?: string }) {
  return <Spinner className={className} />;
}

function Cross({ className }: { className?: string }) {
  return (
    <Glyph className={className}>
      <path d="M6 6l12 12M18 6 6 18" />
    </Glyph>
  );
}

function Chevron({ up, className }: { up: boolean; className?: string }) {
  return (
    <Glyph className={className}>
      <path d={up ? 'M6 15l6-6 6 6' : 'M6 9l6 6 6-6'} />
    </Glyph>
  );
}

const TOAST_ICONS = {
  error: CircleAlert,
  info: Info,
  loading: Loading,
  success: CircleCheck,
  warning: TriangleAlert,
} as const;

/* upstream toast.logic.ts ------------------------------------------------------------- */

/**
 * Base UI toast updates omit `undefined` fields, so callers that need to remove
 * an action must pass a defined `actionProps` whose `children` are empty.
 * Treat that payload (and missing children) as "no visible action".
 */
function hasVisibleToastAction(actionProps: unknown): boolean {
  if (actionProps == null || typeof actionProps !== 'object') return false;
  if (!('children' in actionProps)) return false;
  const children = actionProps.children;
  return children != null && children !== false && children !== '';
}

function shouldHideCollapsedToastContent(visibleIndex: number, visibleCount: number): boolean {
  if (visibleCount <= 1) return false;
  return visibleIndex > 0;
}

type ToastWithLayoutProps = {
  height?: number | null | undefined;
  transitionStatus?: 'starting' | 'ending' | undefined;
};

type VisibleToastLayoutItem<TToast extends object> = {
  toast: TToast;
  visibleIndex: number;
  offsetY: number;
};

function normalizeToastHeight(height: number | null | undefined): number {
  return typeof height === 'number' && Number.isFinite(height) && height > 0 ? height : 0;
}

function buildVisibleToastLayout<TToast extends object>(
  visibleToasts: readonly (TToast & ToastWithLayoutProps)[],
): {
  frontmostHeight: number;
  items: VisibleToastLayoutItem<TToast & ToastWithLayoutProps>[];
} {
  let fullIndex = 0;
  let fullOffsetY = 0;
  let liveIndex = 0;
  let liveOffsetY = 0;

  const items = visibleToasts.map((toast) => {
    const height = normalizeToastHeight(toast.height);
    if (toast.transitionStatus === 'ending') {
      const item = { toast, visibleIndex: fullIndex, offsetY: fullOffsetY };
      fullOffsetY += height;
      fullIndex += 1;
      return item;
    }
    const item = { toast, visibleIndex: liveIndex, offsetY: liveOffsetY };
    fullOffsetY += height;
    fullIndex += 1;
    liveOffsetY += height;
    liveIndex += 1;
    return item;
  });

  const frontmostLiveToast = visibleToasts.find((toast) => toast.transitionStatus !== 'ending');
  return { frontmostHeight: normalizeToastHeight(frontmostLiveToast?.height), items };
}

/* The body ------------------------------------------------------------------------------ */

const ERROR_DESCRIPTION_CLAMP_MIN_CHARS = 180;
function errorDescriptionClampClass(type: unknown, description: unknown): string | undefined {
  if (type !== 'error' || typeof description !== 'string') return undefined;
  if (description.length < ERROR_DESCRIPTION_CLAMP_MIN_CHARS) return undefined;
  return 'line-clamp-4';
}

const EXPANDABLE_PANEL =
  'mt-2 max-h-40 min-h-0 overflow-y-auto overscroll-contain pr-0.5 select-text';

function ToastExpandableSection({
  children,
  labels,
}: {
  children: ReactNode;
  labels: { expand?: string; collapse?: string };
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="min-w-0">
      <button
        aria-expanded={open}
        className="inline-flex cursor-pointer items-center gap-1 rounded-sm border-0 bg-transparent p-0 py-0.5 text-left font-medium text-muted text-small transition-colors duration-(--op-duration) hover:text-ink focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
        onClick={() => setOpen((prev) => !prev)}
        type="button"
      >
        <Chevron up={open} className="size-3.5 shrink-0 opacity-80" />
        {open ? (labels.collapse ?? 'Hide details') : (labels.expand ?? 'Show details')}
      </button>
      {open ? <div className={EXPANDABLE_PANEL}>{children}</div> : null}
    </div>
  );
}

function ToastDescriptionAndExpandable({
  toastData,
  toastDescription,
  toastType,
}: {
  toastData: ToastData | undefined;
  toastDescription: unknown;
  toastType: unknown;
}) {
  const descriptionClassName = cx(
    'm-0 min-w-0 select-text wrap-break-word text-body text-muted',
    errorDescriptionClampClass(toastType, toastDescription),
  );
  const description = (
    <Toast.Description className={descriptionClassName} data-slot="toast-description" />
  );
  if (!toastData?.expandableContent) return description;
  return (
    <>
      {description}
      <ToastExpandableSection labels={toastData.expandableLabels ?? {}}>
        {toastData.expandableContent}
      </ToastExpandableSection>
    </>
  );
}

type ToastIconComponent = (typeof TOAST_ICONS)[keyof typeof TOAST_ICONS];

interface ToastBodyDescriptor {
  readonly Icon: ToastIconComponent | null | undefined;
  readonly stackedActionLayout: boolean;
  readonly actionVariant: ButtonVariant;
  readonly secondaryActionVariant: ButtonVariant;
  readonly hasTrailingControls: boolean;
  readonly inlineContentEndPad: string;
}

function deriveToastBodyDescriptor(toast: {
  readonly type?: string | undefined;
  readonly actionProps?: unknown;
  readonly data?: ToastData | undefined;
}): ToastBodyDescriptor {
  const type = typeOf(toast.type);
  const Icon = type ? TOAST_ICONS[type as keyof typeof TOAST_ICONS] : null;
  const stackedActionLayout =
    hasVisibleToastAction(toast.actionProps) && toast.data?.actionLayout === 'stacked-end';
  const hasTrailingControls =
    hasVisibleToastAction(toast.actionProps) || toast.data?.secondaryActionProps !== undefined;
  return {
    Icon,
    stackedActionLayout,
    actionVariant: toast.data?.actionVariant ?? 'primary',
    secondaryActionVariant: toast.data?.secondaryActionVariant ?? 'secondary',
    hasTrailingControls,
    inlineContentEndPad: hasTrailingControls ? 'pr-6' : 'pr-10',
  };
}

interface ToastBodyProps extends ToastBodyDescriptor {
  readonly actionProps: { readonly children?: ReactNode } | undefined;
  readonly toastData: ToastData | undefined;
  readonly toastDescription: unknown;
  readonly toastType: unknown;
}

function ToastBody({
  stackedActionLayout,
  Icon,
  actionProps,
  actionVariant,
  secondaryActionVariant,
  hasTrailingControls,
  toastData,
  toastDescription,
  toastType,
}: ToastBodyProps) {
  const secondaryActionProps = toastData?.secondaryActionProps;
  const leadingIcon = toastData?.leadingIcon;
  const { className: secondaryActionClassName, ...secondaryActionRest } =
    secondaryActionProps ?? {};

  return (
    <>
      <div
        className={cx(
          'flex min-h-0 min-w-0 flex-col gap-0.5',
          stackedActionLayout ? 'pr-5' : 'flex-1',
        )}
      >
        <div className="flex min-w-0 gap-2">
          {leadingIcon ? (
            <div
              className="flex h-lh w-4 shrink-0 items-center justify-center"
              data-slot="toast-icon"
            >
              {leadingIcon}
            </div>
          ) : Icon ? (
            <div
              className="flex h-lh shrink-0 items-center [&_svg]:pointer-events-none"
              data-slot="toast-icon"
            >
              <Icon className="in-data-[type=error]:text-danger in-data-[type=info]:text-info in-data-[type=success]:text-grow in-data-[type=warning]:text-warn in-data-[type=loading]:opacity-80" />
            </div>
          ) : null}
          <Toast.Title
            className="m-0 min-w-0 wrap-break-word font-medium text-body"
            data-slot="toast-title"
          />
        </div>
        <ToastDescriptionAndExpandable
          toastData={toastData}
          toastDescription={toastDescription}
          toastType={toastType}
        />
      </div>
      {hasTrailingControls ? (
        <div
          className={cx(
            'flex items-center gap-1.5',
            stackedActionLayout ? 'w-full justify-end' : 'shrink-0',
          )}
        >
          {secondaryActionProps ? (
            <Button
              {...secondaryActionRest}
              className={secondaryActionClassName}
              size="xs"
              type="button"
              variant={secondaryActionVariant}
            />
          ) : null}
          {hasVisibleToastAction(actionProps) ? (
            <Toast.Action
              className={buttonClass({ size: 'xs', variant: actionVariant, className: 'shrink-0' })}
              data-slot="toast-action"
            >
              {actionProps?.children}
            </Toast.Action>
          ) : null}
        </div>
      ) : null}
    </>
  );
}

/** Upstream's `ThreadToastVisibleAutoDismiss`: counts only while the window is seen. */
function VisibleAutoDismiss({
  toastId,
  dismissAfterVisibleMs,
}: {
  toastId: ToastId;
  dismissAfterVisibleMs: number | undefined;
}) {
  useEffect(() => {
    if (!dismissAfterVisibleMs || dismissAfterVisibleMs <= 0) return;
    if (typeof window === 'undefined' || typeof document === 'undefined') return;

    let remainingMs = visibleTimeoutRemainingMs.get(toastId) ?? dismissAfterVisibleMs;
    let startedAtMs: number | null = null;
    let timeoutId: number | null = null;
    let closed = false;

    const clearTimer = () => {
      if (timeoutId === null) return;
      window.clearTimeout(timeoutId);
      timeoutId = null;
    };
    const closeToast = () => {
      if (closed) return;
      closed = true;
      visibleTimeoutRemainingMs.delete(toastId);
      toastManager.close(toastId);
    };
    const pause = () => {
      if (startedAtMs === null) return;
      remainingMs = Math.max(0, remainingMs - (Date.now() - startedAtMs));
      startedAtMs = null;
      clearTimer();
      visibleTimeoutRemainingMs.set(toastId, remainingMs);
    };
    const start = () => {
      if (closed || startedAtMs !== null) return;
      if (remainingMs <= 0) {
        closeToast();
        return;
      }
      startedAtMs = Date.now();
      clearTimer();
      timeoutId = window.setTimeout(() => {
        remainingMs = 0;
        startedAtMs = null;
        closeToast();
      }, remainingMs);
    };
    const syncTimer = () => {
      if (document.visibilityState === 'visible' && document.hasFocus()) start();
      else pause();
    };

    syncTimer();
    document.addEventListener('visibilitychange', syncTimer);
    window.addEventListener('focus', syncTimer);
    window.addEventListener('blur', syncTimer);
    return () => {
      document.removeEventListener('visibilitychange', syncTimer);
      window.removeEventListener('focus', syncTimer);
      window.removeEventListener('blur', syncTimer);
      pause();
      clearTimer();
    };
  }, [dismissAfterVisibleMs, toastId]);

  return null;
}

/* The Loams-facing names ------------------------------------------------------------------ */

/** Holds the toasts; the shell mounts it once, with a `<Toaster />` inside. */
export function ToastProvider({ children, ...props }: Toast.Provider.Props) {
  return (
    <Toast.Provider limit={4} timeout={5000} {...props} toastManager={toastManager}>
      {children}
    </Toast.Provider>
  );
}

export interface ToastApi {
  /** Shows a toast and returns its id. */
  add(o: ToastOptions): string;
  close(id: string): void;
  update(id: string, o: Partial<ToastOptions>): void;
}

/** `add({ title, description, type, actionProps })` shows a toast; it returns its id. */
export function useToast(): ToastApi {
  const { add, close, update } = Toast.useToastManager<ToastData>();
  return useMemo(
    () => ({
      add: (o: ToastOptions) => add(normalise(o)),
      close: (id: string) => close(id),
      update: (id: string, o: Partial<ToastOptions>) => update(id, normalise(o)),
    }),
    [add, close, update],
  );
}

const TOAST_ROOT = [
  'absolute right-0 bottom-0 z-[calc(9999-var(--toast-index))] w-full origin-bottom overflow-visible select-none',
  'rounded-lg border border-rule bg-elevated text-body text-ink shadow-toast',
  // Enter and exit on transform and opacity (UI1-1): 220ms out of the bottom edge, the stack
  // moving with it; reduced motion zeroes the tokens.
  '[transition:transform_var(--op-duration-page)_var(--op-ease-out),opacity_var(--op-duration-enter)_var(--op-ease-out),height_var(--op-duration)] motion-reduce:transition-none',
  // A bridge across the gap, so moving between spread-out toasts keeps the stack open.
  'after:absolute after:bottom-full after:left-0 after:h-[calc(var(--toast-gap)+1px)] after:w-full',
  '[--toast-gap:12px] [--toast-peek:12px] [--toast-scale:calc(max(0,1-(var(--toast-index)*.1)))] [--toast-shrink:calc(1-var(--toast-scale))]',
  '[--toast-calc-offset-y:calc(var(--toast-offset-y)*-1+var(--toast-index)*var(--toast-gap)*-1+var(--toast-swipe-movement-y))]',
  'transform-[translateX(var(--toast-swipe-movement-x))_translateY(calc(var(--toast-swipe-movement-y)-(var(--toast-index)*var(--toast-peek))-(var(--toast-shrink)*var(--toast-calc-height))))_scale(var(--toast-scale))]',
  'data-limited:opacity-0',
  'data-expanded:transform-[translateX(var(--toast-swipe-movement-x))_translateY(var(--toast-calc-offset-y))]',
  'data-starting-style:transform-[translateY(calc(100%+16px))]',
  'data-ending-style:opacity-0',
  'data-ending-style:not-data-limited:not-data-swipe-direction:transform-[translateY(calc(100%+16px))]',
  'data-ending-style:data-[swipe-direction=right]:transform-[translateX(calc(var(--toast-swipe-movement-x)+100%+16px))_translateY(var(--toast-calc-offset-y))]',
  'data-ending-style:data-[swipe-direction=down]:transform-[translateY(calc(var(--toast-swipe-movement-y)+100%+16px))]',
].join(' ');

/** Where toasts appear: the bottom right, stacked, newest in front (D1082). */
export function Toaster() {
  const { toasts } = Toast.useToastManager<ToastData>();
  const layout = buildVisibleToastLayout(toasts);

  useEffect(() => {
    const live = new Set(toasts.map((t) => t.id));
    for (const id of visibleTimeoutRemainingMs.keys()) {
      if (!live.has(id)) visibleTimeoutRemainingMs.delete(id);
    }
  }, [toasts]);

  return (
    <Toast.Portal data-slot="toast-portal">
      <Toast.Viewport
        aria-label="Notifications"
        className="fixed right-4 bottom-4 z-[80] flex w-[min(360px,calc(100vw-32px))] [-webkit-app-region:no-drag]"
        data-slot="toast-viewport"
        style={{ '--toast-frontmost-height': `${layout.frontmostHeight}px` } as CSSProperties}
      >
        {layout.items.map(({ toast, visibleIndex, offsetY }) => {
          const hideCollapsedContent = shouldHideCollapsedToastContent(
            visibleIndex,
            layout.items.length,
          );
          const body = deriveToastBodyDescriptor(toast);
          return (
            <Toast.Root
              key={toast.id}
              toast={toast}
              data-type={typeOf(toast.type)}
              swipeDirection={['right', 'down']}
              className={cx(
                TOAST_ROOT,
                // Never `min-h-(--toast-height)`: Base UI measures by forcing `height: auto`,
                // and an old min-height would keep a closed expandable's height.
                visibleIndex > 0
                  ? 'not-data-expanded:h-(--toast-calc-height) not-data-expanded:[--toast-calc-height:var(--toast-frontmost-height)] data-expanded:h-auto data-expanded:[--toast-calc-height:max(var(--toast-frontmost-height,var(--toast-height)),var(--toast-height))]'
                  : 'h-auto [--toast-calc-height:max(var(--toast-frontmost-height,var(--toast-height)),var(--toast-height))]',
              )}
              style={
                {
                  '--toast-index': visibleIndex,
                  '--toast-offset-y': `${offsetY}px`,
                } as CSSProperties
              }
            >
              <VisibleAutoDismiss
                toastId={toast.id}
                dismissAfterVisibleMs={toast.data?.dismissAfterVisibleMs}
              />
              <div
                className={cx(
                  'absolute -top-1.5 -right-1.5 z-20',
                  // Behind the front toast, the dismiss button shows only when the stack spreads.
                  hideCollapsedContent &&
                    '[:not([data-expanded])>&]:pointer-events-none [:not([data-expanded])>&]:opacity-0',
                )}
              >
                <button
                  aria-label="Dismiss"
                  className="inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-pill border border-rule bg-surface p-0 text-muted transition-colors duration-(--op-duration) hover:bg-raised hover:text-ink focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                  data-slot="toast-close"
                  onClick={() => {
                    toast.data?.onClose?.();
                    toastManager.close(toast.id);
                  }}
                  type="button"
                >
                  <Cross className="size-3" />
                </button>
              </div>
              <Toast.Content
                className={cx(
                  // `overflow-x: clip` keeps an expanded details panel from being cut off below.
                  'pointer-events-auto min-h-0 overflow-y-visible pl-3.5 transition-opacity duration-(--op-duration) [overflow-x:clip] data-expanded:opacity-100',
                  body.stackedActionLayout
                    ? 'flex flex-col gap-2 py-2.5 pr-3.5'
                    : cx(
                        'flex items-center justify-between gap-1.5 py-3',
                        body.inlineContentEndPad,
                      ),
                  hideCollapsedContent &&
                    'not-data-expanded:pointer-events-none not-data-expanded:opacity-0',
                )}
              >
                <ToastBody
                  {...body}
                  actionProps={toast.actionProps}
                  toastData={toast.data}
                  toastDescription={toast.description}
                  toastType={typeOf(toast.type)}
                />
              </Toast.Content>
            </Toast.Root>
          );
        })}
      </Toast.Viewport>
    </Toast.Portal>
  );
}

export { hiddenToastActionProps, type StackedToastOptions, stackedToast } from './toast-helpers';
