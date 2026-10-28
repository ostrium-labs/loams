import type { ConsoleAccount, RouterService, SessionService } from '@loams/console-host';
import { Notice, useToast } from '@loams/ui';
import {
  type CSSProperties,
  type FormEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useSyncExternalStore,
} from 'react';
import { errorText } from './model.js';

/**
 * The root of every admin page: the scope its styles live under (admin.css), and
 * the staggered rise the original console's content had.
 */
export function Page({ children }: { children: ReactNode }) {
  return <div className="la la-page">{children}</div>;
}

/**
 * A form in the original console's spacing. Dialog bodies render in a portal, outside
 * the page's `.la` scope, so this carries the scope itself. Without `onSubmit` it is a
 * plain group (a dialog whose footer button acts on click).
 */
export function Form({
  id,
  onSubmit,
  children,
}: {
  id?: string;
  onSubmit?: (event: FormEvent<HTMLFormElement>) => void;
  children: ReactNode;
}) {
  return (
    <div className="la">
      {onSubmit ? (
        <form id={id} className="form" onSubmit={onSubmit}>
          {children}
        </form>
      ) : (
        <div className="form">{children}</div>
      )}
    </div>
  );
}

/** Content in a dialog or another portal, under the pages' style scope. */
export function Scoped({ children }: { children: ReactNode }) {
  return <div className="la">{children}</div>;
}

export function PageHead({
  title,
  eyebrow,
  children,
  actions,
}: {
  title: ReactNode;
  eyebrow?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="page-head">
      <div>
        {eyebrow && <div className="page-eyebrow">{eyebrow}</div>}
        <h1>{title}</h1>
        {children && <div className="page-intro">{children}</div>}
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </div>
  );
}

export function Loading({ label = 'Loading' }: { label?: string }) {
  return (
    <div className="loading" aria-busy="true">
      <span className="sr-only">{label}</span>
      <span className="skeleton" />
      <span className="skeleton" />
      <span className="skeleton short" />
    </div>
  );
}

export function LoadError({ error, retry }: { error: unknown; retry?: () => void }) {
  return (
    <Notice tone="danger" title="This didn't load">
      {typeof error === 'string' ? error : errorText(error)}{' '}
      {retry && (
        <button type="button" className="link-btn" onClick={retry}>
          Try again
        </button>
      )}
    </Notice>
  );
}

/** A link inside the console: a hash route, as the shell's router reads it. */
export function RouteLink({
  to,
  className,
  style,
  children,
}: {
  to: string;
  className?: string;
  style?: CSSProperties;
  children: ReactNode;
}) {
  return (
    <a href={`#${to}`} className={className} style={style}>
      {children}
    </a>
  );
}

/** The page's breadcrumbs (and tab title, the last crumb), as the original console showed them. */
export function useTrail(router: RouterService, crumbs: { label: string; href?: string }[]) {
  const key = JSON.stringify(crumbs);
  // oxlint-disable-next-line react/exhaustive-deps -- keyed on the crumbs' content
  useEffect(() => {
    router.setTrail?.(crumbs);
  }, [key, router]);
}

/** The console session (the person, their org and role), kept current. */
export function useAccount(session: SessionService): ConsoleAccount | undefined {
  return useSyncExternalStore(session.subscribe, () => session.account?.());
}

/** A short confirmation at the bottom right; `danger` for a failure. */
export function useNotify(): (text: string, tone?: 'success' | 'danger') => void {
  const toast = useToast();
  return useCallback(
    (text, tone = 'success') => {
      toast.add({ title: text, type: tone === 'danger' ? 'danger' : 'success' });
    },
    [toast],
  );
}
