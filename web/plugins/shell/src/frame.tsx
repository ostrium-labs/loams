// The shell's frame (design 59, D1078): the sidebar column, the 44px top bar, which is the
// window's drag region (D1083), and the main region, which renders the home slot on the
// mode's home and a page everywhere else. The work area (main, and the inspector and drawer
// from Task 8) is a card on the window. Components get props and closures, never `ctx`.

import {
  type FlagsService,
  MODE_HOME,
  type ModeService,
  type SessionService,
  SHELL_COMMAND_EVENT,
} from '@loams/console-host';
import { Slot, SlotBoundary, useSlot } from '@loams/slots';
import {
  Avatar,
  Badge,
  Empty,
  IconButton,
  Kbd,
  KbdGroup,
  Logo,
  Menu,
  MenuItem,
  MenuLabel,
  MenuSeparator,
  SelectMenu,
} from '@loams/ui';
import { ChevronsUpDown, GitBranch, LogOut, PanelLeft, X } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useState, useSyncExternalStore } from 'react';
import { useMode } from './mode-switcher.js';
import { inMode, useLocation, useModeFilter } from './nav.js';
import { currentProject, pageAt, pageEntries } from './pages.js';
import type { Crumb, HashRouter } from './router.js';

/** The sidebar's saved state, limits and default width (D1078: 232px, 200–320). */
export const SIDEBAR = { key: 'loams.shell.sidebar', min: 200, max: 320, size: 232 } as const;

export interface SidebarState {
  open: boolean;
  width: number;
}

const clampSidebar = (n: number) => Math.min(SIDEBAR.max, Math.max(SIDEBAR.min, Math.round(n)));

/** The saved sidebar state; open at 232px when nothing (or nothing readable) is saved. */
export function readSidebar(): SidebarState {
  try {
    const raw = JSON.parse(
      localStorage.getItem(SIDEBAR.key) ?? 'null',
    ) as Partial<SidebarState> | null;
    return {
      open: raw?.open !== false,
      width: typeof raw?.width === 'number' ? clampSidebar(raw.width) : SIDEBAR.size,
    };
  } catch {
    return { open: true, width: SIDEBAR.size };
  }
}

/** The sidebar's open state and width, saved per device. */
export function useSidebarState(): [SidebarState, (patch: Partial<SidebarState>) => void] {
  const [state, setState] = useState(readSidebar);
  const update = useCallback((patch: Partial<SidebarState>) => {
    setState((prev) => {
      const next = { ...prev, ...patch, width: clampSidebar(patch.width ?? prev.width) };
      try {
        localStorage.setItem(SIDEBAR.key, JSON.stringify(next));
      } catch {
        // Storage blocked: the sidebar still works this session.
      }
      return next;
    });
  }, []);
  return [state, update];
}

const NARROW = '(max-width: 959px)';

/** Under 960px the sidebar is a sheet (and, from Task 8, the inspector an overlay). */
export function useNarrow(): boolean {
  return useSyncExternalStore(
    (cb) => {
      const mq = globalThis.matchMedia?.(NARROW);
      mq?.addEventListener?.('change', cb);
      return () => mq?.removeEventListener?.('change', cb);
    },
    () => globalThis.matchMedia?.(NARROW).matches ?? false,
    () => false,
  );
}

/** Sends a shell command to whoever handles it (Studio's `chat.new`); else runs `fallback`. */
export function runShellCommand(
  detail: string,
  fallback: () => void,
  target: EventTarget = globalThis,
): void {
  const event = new CustomEvent(SHELL_COMMAND_EVENT, { detail, cancelable: true });
  target.dispatchEvent(event);
  if (!event.defaultPrevented) fallback();
}

function isMac(): boolean {
  try {
    return /mac|iphone|ipad|ipod/i.test(navigator.platform);
  } catch {
    // No navigator (tests, workers): the Ctrl spelling.
    return false;
  }
}

/** The modifier as a key cap shows it: ⌘ on macOS, Ctrl elsewhere. */
export const modKey = (mac = isMac()) => (mac ? '⌘' : 'Ctrl');

/** A top bar tooltip: what the control does, then its shortcut (Mod plus `keys`). */
export function ShortcutHint({ label, keys }: { label: string; keys: string }) {
  return (
    <span className="lc-tip">
      {label}
      <KbdGroup aria-hidden="true">
        <Kbd>{modKey()}</Kbd>
        <Kbd>{keys}</Kbd>
      </KbdGroup>
    </span>
  );
}

type BarSlot =
  | 'shell.topbar.left'
  | 'shell.topbar.center'
  | 'shell.topbar.right'
  | 'shell.header.server';

/** A top bar slot's entries of the current mode. */
function Entries({ name, mode }: { name: BarSlot; mode?: ModeService }) {
  const entries = useSlot(name);
  const filter = useModeFilter(mode);
  return (
    <>
      {entries
        .filter((e) => inMode(e, filter))
        .map((entry) => {
          const Entry = entry.component;
          return (
            <SlotBoundary key={entry.id} plugin={entry.plugin}>
              <Entry />
            </SlotBoundary>
          );
        })}
    </>
  );
}

/**
 * The top bar (D1078, D1083): the sidebar toggle, the context pills and the breadcrumbs,
 * the palette hint, and the tools. The bar is the window's drag region; its controls are
 * `no-drag` (console.css), and `--lc-traffic` / `--lc-wco-end` keep the OS window controls
 * clear (Task 13 sets them).
 */
export function TopBar({
  router,
  flags,
  mode,
  session,
  sidebarOpen,
  onToggleSidebar,
  onOpenPalette,
  tools,
  trailing,
}: {
  router: HashRouter;
  flags?: FlagsService;
  mode?: ModeService;
  /** The session's environments: the branch pill in Console (W21). */
  session?: SessionService;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  onOpenPalette: () => void;
  /** The mode switcher, the appearance menu and the account (Layout's). */
  tools: ReactNode;
  /** The inspector and drawer toggles (Task 8). */
  trailing?: ReactNode;
}) {
  const mac = isMac();
  const current = useMode(mode);
  return (
    <header className="lc-topbar" data-drag-region="">
      <IconButton
        size="sm"
        className="lc-bar-btn lc-sidebar-toggle"
        label={sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}
        tooltip={<ShortcutHint label={sidebarOpen ? 'Hide sidebar' : 'Show sidebar'} keys="B" />}
        aria-controls="lc-side"
        aria-expanded={sidebarOpen}
        aria-keyshortcuts="Control+B Meta+B"
        onClick={onToggleSidebar}
      >
        <PanelLeft strokeWidth={1.75} aria-hidden="true" />
      </IconButton>
      <div className="lc-topbar-start">
        <Entries name="shell.topbar.left" mode={mode} />
        <Entries name="shell.header.server" mode={mode} />
        {session && current === 'console' && <EnvironmentSwitcher session={session} />}
        <Crumbs router={router} flags={flags} mode={mode} />
      </div>
      <div className="lc-topbar-center">
        <Entries name="shell.topbar.center" mode={mode} />
        <button
          type="button"
          className="lc-palette-hint"
          aria-label="Search or run a command"
          aria-keyshortcuts="Control+K Meta+K"
          onClick={onOpenPalette}
        >
          <span className="lc-palette-hint-text">Search or run a command</span>
          <KbdGroup aria-hidden="true">
            <Kbd>{modKey(mac)}</Kbd>
            <Kbd>K</Kbd>
          </KbdGroup>
        </button>
      </div>
      <div className="lc-topbar-end">
        <Entries name="shell.topbar.right" mode={mode} />
        <div className="lc-overlays">
          <Slot name="shell.overlay" props={{}} />
        </div>
        {tools}
        {trailing}
      </div>
    </header>
  );
}

/** The home slot on the mode's home route (W11); the page outlet everywhere else. */
export function MainRegion({ router, mode }: { router: HashRouter; mode?: ModeService }) {
  const location = useLocation(router);
  const current = useMode(mode);
  const filter = useModeFilter(mode);
  // Studio's home entry is `meta.mode: 'studio'`, so Console's "/" stays the Overview page.
  const hasHome = useSlot('shell.home').some((e) => inMode(e, filter));
  const home = hasHome && location.path === MODE_HOME[current];
  useEffect(() => {
    if (home) document.title = 'Loams';
  }, [home]);
  if (home) {
    return (
      <div className="lc-home">
        <Slot name="shell.home" props={{ params: location.params }} />
      </div>
    );
  }
  return <Outlet router={router} />;
}

/**
 * The page's place (D1080, no sections): the instance (Studio in Studio mode), the page the
 * route is under when it is not the page itself, then the page. A page's own trail wins.
 */
export function Crumbs({
  router,
  flags,
  mode,
}: {
  router: HashRouter;
  flags?: FlagsService;
  mode?: ModeService;
}) {
  const sections = useSlot('shell.nav.section');
  const legacy = useSlot('console.nav');
  const location = useLocation(router);
  const trail = useSyncExternalStore(router.subscribe, () => router.trail());
  const current = useMode(mode);
  const title = location.pageId ? router.title() : undefined;
  if (trail && trail.length > 0) return <Trail crumbs={trail} />;
  const home = pageAt(pageEntries(sections, legacy, currentProject(location.path)), location.path);
  const root =
    mode && current === 'studio'
      ? { label: 'Studio', href: MODE_HOME.studio }
      : { label: flags?.instanceName || 'Loams', href: '/' };
  const crumbs: { label: string; href?: string }[] = [root];
  if (home && home.href !== location.path && home.href !== root.href) {
    crumbs.push({ label: home.label, href: home.href });
  }
  const last = title ?? home?.label ?? 'Not found';
  if (location.path !== root.href || last !== root.label) crumbs.push({ label: last });
  return <Trail crumbs={crumbs} />;
}

/** The branch pill in Console (W21): the environment the session reads. */
export function EnvironmentSwitcher({ session }: { session: SessionService }) {
  useSyncExternalStore(session.subscribe, () => session.environment?.());
  const environments = session.environments?.() ?? [];
  const current = session.environment?.();
  if (!current || environments.length === 0) return null;
  return (
    <div className="lc-env-pill">
      <SelectMenu
        label="Environment"
        value={current.id}
        onValueChange={(id) => session.select(id)}
        options={environments.map((e) => ({ value: e.id, label: e.name }))}
        className="lc-pill-trigger"
        leading={<GitBranch size={13} strokeWidth={1.75} aria-hidden="true" />}
        icon={<ChevronsUpDown size={13} />}
      />
    </div>
  );
}

export function Trail({ crumbs }: { crumbs: Crumb[] }) {
  return (
    <nav aria-label="Breadcrumb" className="lc-trail">
      <ol>
        {crumbs.map((c, i) => {
          const last = i === crumbs.length - 1;
          return (
            // oxlint-disable-next-line react/no-array-index-key -- crumbs are positional
            <li key={i} aria-current={last ? 'page' : undefined}>
              {!last && c.href ? <a href={`#${c.href}`}>{c.label}</a> : <span>{c.label}</span>}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

/** The current page, or "Nothing here" when no plugin serves the path. */
export function Outlet({ router }: { router: HashRouter }) {
  const location = useLocation(router);
  const trail = useSyncExternalStore(router.subscribe, () => router.trail());
  const title = trail?.at(-1)?.label ?? (location.pageId ? router.title() : undefined);
  useEffect(() => {
    document.title = title ? `${title} · Loams` : 'Loams';
  }, [title]);
  if (!location.pageId) {
    return (
      <Empty title="Nothing here">
        No plugin serves <code>{location.path}</code>. It may be disabled, or waiting for an API
        this instance does not serve.
      </Empty>
    );
  }
  return (
    <Slot
      name="console.page"
      slotKey={location.pageId}
      props={{ params: location.params }}
      fallback={<Empty title="Loading…" />}
    />
  );
}

const KIND_LABEL: Record<number, string> = { 1: 'User', 2: 'Service account', 3: 'Agent' };

const ROLE_LABEL: Record<number, string> = { 1: 'owner', 2: 'admin', 3: 'member' };

export function UserMenu({
  session,
  router,
  local,
}: {
  session: SessionService;
  router: HashRouter;
  /** The desktop's own engine: there is no session to end. */
  local?: boolean;
}) {
  useSyncExternalStore(session.subscribe, () => session.principal());
  const account = useSyncExternalStore(session.subscribe, () => session.account?.());
  const who = session.principal();
  if (!who && !account) return null;
  const name = account?.user?.name || who?.displayName || 'Signed in';
  const email = account?.user?.email || who?.email;
  const role = account ? ROLE_LABEL[account.role] : undefined;
  const signOut = session.signOut && !local ? session.signOut : undefined;
  return (
    <Menu
      label="Account"
      trigger={
        <button type="button" className="lc-user-btn" aria-label={`Account: ${name}`}>
          <Avatar name={name} size={28} />
        </button>
      }
    >
      <MenuLabel>
        <strong className="text-ink">{name}</strong>
        {email && <span>{email}</span>}
        <span>
          <Badge>{role ?? (who ? KIND_LABEL[who.kind] : undefined) ?? 'Signed in'}</Badge>
        </span>
      </MenuLabel>
      {signOut && (
        <>
          <MenuSeparator />
          <MenuItem
            onSelect={() => {
              void signOut().then(() => router.navigate('/sign-in'));
            }}
          >
            <LogOut size={15} aria-hidden="true" /> Sign out
          </MenuItem>
        </>
      )}
    </Menu>
  );
}

/** The wordmark, at the top of the sidebar. */
export function Brand({
  home = '/',
  onClose,
}: {
  home?: string;
  /** A close button after the badges (none in the frame: the sheet and the top bar close it). */
  onClose?: () => void;
}) {
  return (
    <div className="lc-brand">
      <a href={`#${home}`} aria-label="Loams home" className="lc-brand-link">
        <Logo height={18} />
      </a>
      {onClose && (
        <button
          type="button"
          className="lc-side-close"
          aria-label="Close navigation"
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      )}
    </div>
  );
}
