// The cloud-console layout: product nav on the left, a header (server
// switcher, agent toggle, user menu), the page, and a resizable dock on the
// right (D666). Components get props and closures, never `ctx`.

import type { SessionService } from '@loams/console-host';
import { Slot, type SlotEntry, useSlot } from '@loams/slots';
import { Empty, Logo, Mark } from '@loams/ui';
import { PanelRight } from 'lucide-react';
import {
  type KeyboardEvent,
  type PointerEvent,
  useCallback,
  useEffect,
  useState,
  useSyncExternalStore,
} from 'react';
import { iconFor } from './icons.js';
import type { HashRouter } from './router.js';

export const DOCK_MIN = 320;
export const DOCK_MAX = 720;
export const DOCK_DEFAULT = 400;
export const DOCK_KEY = 'loams.shell.dock';
const DOCK_STEP = 24;

/** The nav sections, in display order. Anything else follows them. */
export const SECTIONS = ['Data', 'Compute', 'Integrate', 'Organisation'] as const;

interface NavItem {
  key: string;
  label: string;
  href: string;
  icon?: string;
  group: string;
  order: number;
}

/**
 * Where the pages that predate `shell.nav.section` (registered through
 * `router.page(... nav)`) land, by path. Orders sit in the bands the product
 * pages leave free: Data 0 Overview, 10 Data, 20 Postgres, 30 WeSQL, 40 Live.
 */
const LEGACY: Record<string, Pick<NavItem, 'group' | 'icon' | 'order'>> = {
  '/': { group: 'Data', icon: 'overview', order: 0 },
  '/namespaces': { group: 'Data', icon: 'namespaces', order: 15 },
  '/approvals': { group: 'Organisation', icon: 'approvals', order: 50 },
};

/** The nav model from both slots, grouped in section order and sorted by `order`. */
export function navSections(
  sections: SlotEntry<'shell.nav.section'>[],
  legacy: SlotEntry<'console.nav'>[],
): [string, NavItem[]][] {
  const items: NavItem[] = [];
  for (const e of sections) {
    items.push({
      key: `s:${e.meta?.id ?? e.id}`,
      label: e.meta?.label ?? e.meta?.id ?? '',
      href: e.meta?.href ?? '/',
      icon: e.meta?.icon,
      group: e.meta?.group ?? 'Organisation',
      order: e.order,
    });
  }
  for (const e of legacy) {
    const href = e.meta?.href ?? '/';
    const known = LEGACY[href];
    const group = e.meta?.group ?? '';
    items.push({
      key: `c:${e.id}`,
      label: e.meta?.label ?? href,
      href,
      icon: known?.icon,
      // Unmapped legacy pages join Organisation, after the first-party ones.
      group:
        known?.group ?? ((SECTIONS as readonly string[]).includes(group) ? group : 'Organisation'),
      order: known?.order ?? 1000 + e.order,
    });
  }
  const byGroup = new Map<string, NavItem[]>();
  for (const item of items) byGroup.set(item.group, [...(byGroup.get(item.group) ?? []), item]);
  const rank = (g: string) => {
    const i = (SECTIONS as readonly string[]).indexOf(g);
    return i < 0 ? SECTIONS.length : i;
  };
  return [...byGroup]
    .sort((a, b) => rank(a[0]) - rank(b[0]))
    .map(([g, list]) => [g, list.sort((a, b) => a.order - b.order)]);
}

function Nav({ router }: { router: HashRouter }) {
  const sections = useSlot('shell.nav.section');
  const legacy = useSlot('console.nav');
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  return (
    <nav className="lc-nav" aria-label="Console">
      {navSections(sections, legacy).map(([group, items]) => (
        <div key={group} className="lc-nav-group">
          <p className="lc-nav-heading">{group}</p>
          <ul>
            {items.map((item) => {
              const active =
                location.path === item.href ||
                (item.href !== '/' && location.path.startsWith(`${item.href}/`));
              const Icon = iconFor(item.icon);
              return (
                <li key={item.key}>
                  <a
                    href={`#${item.href}`}
                    aria-current={active ? 'page' : undefined}
                    title={item.label}
                  >
                    <Icon aria-hidden="true" size={18} strokeWidth={1.75} />
                    <span className="lc-nav-label">{item.label}</span>
                  </a>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </nav>
  );
}

function Outlet({ router }: { router: HashRouter }) {
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  const title = location.pageId ? router.title() : undefined;
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

const clampWidth = (n: number) => Math.min(DOCK_MAX, Math.max(DOCK_MIN, Math.round(n)));

interface DockState {
  open: boolean;
  width: number;
}

function readDock(): DockState {
  try {
    const raw = JSON.parse(localStorage.getItem(DOCK_KEY) ?? 'null') as Partial<DockState> | null;
    return {
      open: raw?.open === true,
      width: typeof raw?.width === 'number' ? clampWidth(raw.width) : DOCK_DEFAULT,
    };
  } catch {
    return { open: false, width: DOCK_DEFAULT };
  }
}

function useDock() {
  const [dock, setDock] = useState<DockState>(readDock);
  const update = useCallback((patch: Partial<DockState>) => {
    setDock((prev) => {
      const next = { ...prev, ...patch };
      try {
        localStorage.setItem(DOCK_KEY, JSON.stringify(next));
      } catch {
        // Storage blocked (private window): the dock still works this session.
      }
      return next;
    });
  }, []);
  return [dock, update] as const;
}

function DockHandle({ width, onWidth }: { width: number; onWidth: (w: number) => void }) {
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const move = (ev: globalThis.PointerEvent) => onWidth(window.innerWidth - ev.clientX);
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const grow = e.key === 'ArrowLeft' ? 1 : e.key === 'ArrowRight' ? -1 : 0;
    if (grow) {
      e.preventDefault();
      onWidth(width + grow * DOCK_STEP);
    } else if (e.key === 'Home') onWidth(DOCK_MIN);
    else if (e.key === 'End') onWidth(DOCK_MAX);
  };
  return (
    // biome-ignore lint/a11y/useSemanticElements: a focusable window splitter has no native element
    <div
      className="lc-dock-handle"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize agent panel"
      aria-valuemin={DOCK_MIN}
      aria-valuemax={DOCK_MAX}
      aria-valuenow={width}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
    />
  );
}

function UserMenu({ session }: { session: SessionService }) {
  useSyncExternalStore(session.subscribe, () => session.principal());
  const who = session.principal();
  const [open, setOpen] = useState(false);
  if (!who) return null;
  const name = who.displayName || 'Signed in';
  const initials =
    name
      .split(/\s+/)
      .map((w) => w[0] ?? '')
      .join('')
      .slice(0, 2)
      .toUpperCase() || '?';
  return (
    <div className="lc-user">
      <button
        type="button"
        className="lc-avatar"
        aria-haspopup="true"
        aria-expanded={open}
        aria-label={`Account: ${name}`}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => e.key === 'Escape' && setOpen(false)}
      >
        {initials}
      </button>
      {open && (
        <div className="lc-user-pop">
          <strong>{name}</strong>
          <span className="lc-muted">Signed in</span>
        </div>
      )}
    </div>
  );
}

export function Layout({ router, session }: { router: HashRouter; session: SessionService }) {
  const hasDock = useSlot('shell.dock.right').length > 0;
  const [dock, setDock] = useDock();
  const open = hasDock && dock.open;
  const toggle = useCallback(() => setDock({ open: !dock.open }), [dock.open, setDock]);

  useEffect(() => {
    if (!hasDock) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'j') {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [hasDock, toggle]);

  return (
    <div className="lc-shell">
      <div className="lc-side">
        <a className="lc-brand" href="#/" aria-label="Loams home">
          <span className="lc-brand-word">
            <Logo />
          </span>
          <span className="lc-brand-mark">
            <Mark size={28} />
          </span>
        </a>
        <Nav router={router} />
      </div>
      <div className="lc-body">
        <header className="lc-header">
          <div className="lc-header-start">
            <Slot name="shell.header.server" props={{}} />
          </div>
          <div className="lc-overlays">
            <Slot name="shell.overlay" props={{}} />
          </div>
          {hasDock && (
            <button
              type="button"
              className="lc-icon-btn"
              aria-expanded={open}
              aria-controls="lc-dock"
              aria-keyshortcuts="Control+J Meta+J"
              aria-label="Agent"
              title="Agent (Ctrl/Cmd+J)"
              onClick={toggle}
            >
              <PanelRight aria-hidden="true" size={18} strokeWidth={1.75} />
              <span className="lc-icon-btn-text">Agent</span>
            </button>
          )}
          <UserMenu session={session} />
        </header>
        <div className="lc-content" style={{ ['--lc-dock-width' as string]: `${dock.width}px` }}>
          <main className="lc-main">
            <Outlet router={router} />
          </main>
          {open && (
            <aside className="lc-dock" id="lc-dock" aria-label="Agent panel">
              <DockHandle width={dock.width} onWidth={(w) => setDock({ width: clampWidth(w) })} />
              <div className="lc-dock-body">
                <Slot name="shell.dock.right" props={{}} />
              </div>
            </aside>
          )}
        </div>
      </div>
    </div>
  );
}
