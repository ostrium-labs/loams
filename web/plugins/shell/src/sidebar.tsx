// The shell's sidebar (design 59, D1078–D1080): the brand, the server indicator and
// "New thread" on top; projects and their threads (`shell.sidebar.threads`, Task 9) and the
// project's pinned Pages in the middle (Settings' own list on a Settings page); Settings, the
// docs link and the version at the foot. 232px by default, resizable from 200 to 320px and
// collapsible (⌘B); below 960px it is a sheet. The palette reaches every page (D1080).

import { type FlagsService, MODE_HOME, type ModeService } from '@loams/console-host';
import { Slot, useSlot } from '@loams/slots';
import {
  AnimatePresence,
  Menu,
  MenuCheckboxItem,
  MOTION_DURATION,
  MOTION_EASE,
  type MotionPreset,
  motion,
  SelectionGroup,
  SelectionIndicator,
  Sheet,
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  useMotionPreset,
} from '@loams/ui';
import {
  ArrowLeft,
  BookOpen,
  ChevronRight,
  LayoutList,
  Pin,
  Settings,
  SquarePen,
} from 'lucide-react';
import { type KeyboardEvent, type ReactNode, useCallback, useRef, useState } from 'react';
import { usePinned } from './all-pages.js';
import { Brand, runShellCommand, SIDEBAR } from './frame.js';
import { iconFor } from './icons.js';
import { useMode } from './mode-switcher.js';
import { useLocation } from './nav.js';
import {
  ALL_PAGES_PATH,
  currentProject,
  isSettingsPage,
  type PageEntry,
  pageAt,
  pageEntries,
  pinnedPages,
  settingsHome,
  settingsSections,
  togglePinned,
} from './pages.js';
import { usePanelToggle } from './panel-motion.js';
import type { HashRouter } from './router.js';

export function ShellSidebar({
  router,
  flags,
  mode,
  width,
  onWidth,
  open,
  narrow,
  onClose,
}: {
  router: HashRouter;
  flags?: FlagsService;
  mode?: ModeService;
  width: number;
  onWidth: (w: number) => void;
  open: boolean;
  narrow: boolean;
  onClose: () => void;
}) {
  const current = useMode(mode);
  const hasThreads = useSlot('shell.sidebar.threads').length > 0;
  const newThread = () =>
    runShellCommand('chat.new', () =>
      mode ? mode.set('studio', { navigate: MODE_HOME.studio }) : router.navigate(MODE_HOME.studio),
    );
  const body = (
    <>
      <SidebarHeader className="lc-side-head">
        <Brand home={MODE_HOME[current]} />
        <Slot name="shell.sidebar.server" props={{}} />
        <button
          type="button"
          className="lc-new-thread"
          onClick={newThread}
          aria-keyshortcuts="Control+N Meta+N"
        >
          <SquarePen size={15} strokeWidth={1.75} aria-hidden="true" /> New thread
        </button>
      </SidebarHeader>
      <SidebarContent>
        {hasThreads && (
          <section aria-label="Threads" className="lc-side-threads">
            <Slot name="shell.sidebar.threads" props={{}} />
          </section>
        )}
        <Navigation router={router} />
      </SidebarContent>
      <SidebarFooter className="lc-side-foot">
        <SettingsLink router={router} />
        <a href="https://loams.dev/docs" target="_blank" rel="noreferrer">
          <BookOpen size={15} aria-hidden="true" /> Documentation
        </a>
        <span className="lc-side-version">Loams {flags?.serverVersion || 'console'}</span>
      </SidebarFooter>
    </>
  );
  if (narrow) {
    return (
      <Sheet open={open} onClose={onClose} title="Navigation" side="left">
        <nav id="lc-side" aria-label="Sidebar" className="lc-side lc-side-sheet">
          {body}
        </nav>
      </Sheet>
    );
  }
  return (
    <SidebarColumn open={open} width={width}>
      {(shown) => (
        <Sidebar
          label="Sidebar"
          width={width}
          onWidthChange={onWidth}
          min={SIDEBAR.min}
          max={SIDEBAR.max}
          open={shown}
          className="lc-side"
        >
          {body}
        </Sidebar>
      )}
    </SidebarColumn>
  );
}

/** The resize handle's width, beside the sidebar inside its column. */
const HANDLE = 1;

/**
 * The sidebar's column: the box the top bar's toggle controls. It snaps open and shut; with
 * Settings › Appearance › Motion on, it slides its width on the panel curve (UI2), the
 * sidebar inside keeping its own width so nothing in it reflows. A drag stays instant.
 */
function SidebarColumn({
  open,
  width,
  children,
}: {
  open: boolean;
  width: number;
  /** The sidebar, given whether to render (open, or still closing). */
  children: (shown: boolean) => ReactNode;
}) {
  const toggle = usePanelToggle(open);
  return (
    <motion.div
      id="lc-side"
      className="lc-side-col"
      initial={false}
      animate={{ width: open ? width + HANDLE : 0 }}
      transition={toggle.transition}
      onAnimationComplete={toggle.done}
      inert={toggle.closing || undefined}
    >
      {children(toggle.shown)}
    </motion.div>
  );
}

/** The sidebar's pages and their location, re-read on navigation. */
function usePages(router: HashRouter) {
  const sections = useSlot('shell.nav.section');
  const legacy = useSlot('console.nav');
  const location = useLocation(router);
  const project = currentProject(location.path);
  const all = pageEntries(sections, legacy, project);
  return { all, project, location };
}

/**
 * The middle of the sidebar (D1080): the project's pinned Pages, or, on a Settings page,
 * the Settings list with a way back to where the person was.
 */
function Navigation({ router }: { router: HashRouter }) {
  const { all, project, location } = usePages(router);
  const here = pageAt(all, location.path);
  const back = useRef('/');
  // The home route stays in Pages even where it is the org's home (`org-home` at `/`).
  const settings = here !== undefined && here.href !== '/' && isSettingsPage(here);
  if (!settings) back.current = location.path;
  return settings ? (
    <SettingsNav pages={all.filter(isSettingsPage)} current={here.id} back={back.current} />
  ) : (
    <PagesSection
      all={all}
      project={project}
      current={here?.id}
      allCurrent={location.path === ALL_PAGES_PATH}
    />
  );
}

/** The sidebar's open and shut groups, saved per device (every group starts open). */
const COLLAPSED_KEY = 'loams.shell.nav.collapsed';

function readCollapsed(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '[]') as unknown;
    return Array.isArray(raw) ? raw.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

function useGroupOpen(id: string): [boolean, (open: boolean) => void] {
  const [open, setOpenState] = useState(() => !readCollapsed().includes(id));
  const setOpen = useCallback(
    (next: boolean) => {
      setOpenState(next);
      try {
        const rest = readCollapsed().filter((g) => g !== id);
        localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next ? rest : [...rest, id]));
      } catch {
        // Storage blocked: the group still opens and shuts this session.
      }
    },
    [id],
  );
  return [open, setOpen];
}

/** A group's rows coming into view: a short fade down from the heading (UI2). */
const DISCLOSE: MotionPreset = {
  initial: { opacity: 0, y: -4 },
  animate: { opacity: 1, y: 0 },
  exit: undefined,
  transition: { duration: MOTION_DURATION.enter, ease: MOTION_EASE.out },
};

/**
 * A group with a heading that opens and shuts it (a disclosure button: aria-expanded and
 * aria-controls). Opening fades the rows in; shutting is instant, and nothing below waits.
 */
function NavGroup({
  id,
  title,
  level,
  action,
  className,
  children,
}: {
  id: string;
  title: string;
  level: 2 | 3;
  action?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useGroupOpen(id);
  const preset = useMotionPreset(DISCLOSE);
  const Heading = level === 2 ? 'h2' : 'h3';
  const body = `${id}-body`;
  return (
    <div className={className} data-open={open ? '' : undefined}>
      <div className={level === 2 ? 'lc-pages-head' : 'lc-settings-sub'}>
        <Heading id={`${id}-title`}>
          <button
            type="button"
            className="lc-nav-disclosure"
            aria-expanded={open}
            aria-controls={body}
            data-nav-row=""
            onClick={() => setOpen(!open)}
          >
            {title}
            <ChevronRight className="lc-nav-chevron" size={12} strokeWidth={2} aria-hidden="true" />
          </button>
        </Heading>
        {action}
      </div>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div key="body" id={body} className="lc-nav-body" {...preset}>
            {children}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/**
 * The arrow keys move between the sidebar's rows (links and group headings), Home and End
 * jump to the first and last; on a group heading, Right opens it and Left shuts it. Tab
 * still walks every control.
 */
function onNavKeys(e: KeyboardEvent<HTMLElement>) {
  if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  const active = document.activeElement as HTMLElement | null;
  if (!active?.matches('[data-nav-row]')) return;
  if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
    const expanded = active.getAttribute('aria-expanded');
    if (expanded === null) return;
    if ((e.key === 'ArrowRight') !== (expanded === 'true')) {
      e.preventDefault();
      active.click();
    }
    return;
  }
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
  const rows = [...e.currentTarget.querySelectorAll<HTMLElement>('[data-nav-row]')];
  const i = rows.indexOf(active);
  if (i < 0) return;
  e.preventDefault();
  const next =
    e.key === 'Home'
      ? 0
      : e.key === 'End'
        ? rows.length - 1
        : Math.min(rows.length - 1, Math.max(0, i + (e.key === 'ArrowDown' ? 1 : -1)));
  rows[next]?.focus();
}

/** The current page's chip, which slides to the next current page (UI2). */
function CurrentChip() {
  return <SelectionIndicator layoutId="lc-nav-current" className="lc-nav-indicator" />;
}

function PageLink({ page, current }: { page: PageEntry; current: boolean }) {
  const Icon = iconFor(page.icon);
  return (
    <li>
      <a
        className="lc-page-link"
        href={`#${page.href}`}
        aria-current={current ? 'page' : undefined}
        title={page.label}
        data-nav-row=""
      >
        {current && <CurrentChip />}
        <Icon aria-hidden="true" size={15} strokeWidth={1.75} />
        <span className="lc-page-label">{page.label}</span>
      </a>
    </li>
  );
}

function PagesSection({
  all,
  project,
  current,
  allCurrent,
}: {
  all: PageEntry[];
  project: string | undefined;
  current?: string;
  allCurrent: boolean;
}) {
  const pinned = usePinned(project);
  const shown = pinnedPages(all, pinned);
  const pinnable = all.filter((p) => !isSettingsPage(p));
  return (
    <SelectionGroup>
      <section className="lc-pages" aria-labelledby="lc-pages-title" onKeyDown={onNavKeys}>
        <NavGroup
          id="lc-pages"
          title="Pages"
          level={2}
          action={
            <Menu
              label="Pin pages to the sidebar"
              trigger={
                <button
                  type="button"
                  className="lc-pages-edit"
                  aria-label="Edit pages"
                  title="Pin or unpin pages"
                >
                  <Pin size={13} strokeWidth={1.75} aria-hidden="true" />
                </button>
              }
            >
              {pinnable.map((p) => (
                <MenuCheckboxItem
                  key={p.id}
                  checked={pinned.includes(p.id)}
                  onCheckedChange={() => togglePinned(project, p.id)}
                >
                  {p.label}
                </MenuCheckboxItem>
              ))}
            </Menu>
          }
        >
          {shown.length > 0 ? (
            <ul aria-labelledby="lc-pages-title">
              {shown.map((p) => (
                <PageLink key={p.id} page={p} current={p.id === current} />
              ))}
            </ul>
          ) : (
            <div className="lc-pages-empty">
              <Pin size={14} strokeWidth={1.75} aria-hidden="true" />
              <p>
                <strong>No pinned pages</strong>
                <span>Pin the pages you use from All pages or the palette.</span>
              </p>
            </div>
          )}
          {/* UX-2: every page, pinned or not, one click away. */}
          <a
            className="lc-page-link lc-pages-all"
            href={`#${ALL_PAGES_PATH}`}
            aria-current={allCurrent ? 'page' : undefined}
            data-nav-row=""
          >
            {allCurrent && <CurrentChip />}
            <LayoutList aria-hidden="true" size={15} strokeWidth={1.75} />
            <span className="lc-page-label">All pages</span>
          </a>
        </NavGroup>
      </section>
    </SelectionGroup>
  );
}

function SettingsNav({
  pages,
  current,
  back,
}: {
  pages: PageEntry[];
  current: string;
  back: string;
}) {
  return (
    <SelectionGroup>
      <nav aria-label="Settings" className="lc-settings-nav" onKeyDown={onNavKeys}>
        <a className="lc-page-link" href={`#${back}`} data-nav-row="">
          <ArrowLeft size={15} strokeWidth={1.75} aria-hidden="true" />
          <span className="lc-page-label">Back</span>
        </a>
        <h2 className="lc-pages-head">Settings</h2>
        {settingsSections(pages).map((s) => (
          <NavGroup
            key={s.section}
            id={`lc-settings-${s.section}`}
            title={s.section}
            level={3}
            className="lc-settings-group"
          >
            <ul aria-labelledby={`lc-settings-${s.section}-title`}>
              {s.pages.map((p) => (
                <PageLink key={p.id} page={p} current={p.id === current} />
              ))}
            </ul>
          </NavGroup>
        ))}
      </nav>
    </SelectionGroup>
  );
}

/** The foot's way into Settings, where ⌘, also goes (D1081). */
function SettingsLink({ router }: { router: HashRouter }) {
  const { all } = usePages(router);
  const home = settingsHome(all);
  if (!home) return null;
  return (
    <a href={`#${home.href}`} aria-keyshortcuts="Control+Comma Meta+Comma">
      <Settings size={15} strokeWidth={1.75} aria-hidden="true" /> Settings
    </a>
  );
}
