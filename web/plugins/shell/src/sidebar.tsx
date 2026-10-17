// The shell's sidebar (design 59, D1078–D1080): the brand, the server indicator and
// "New thread" on top; projects and their threads (`shell.sidebar.threads`, Task 9) and the
// project's pinned Pages in the middle (Settings' own list on a Settings page); Settings, the
// docs link and the version at the foot. 232px by default, resizable from 200 to 320px and
// collapsible (⌘B); below 960px it is a sheet. The palette reaches every page (D1080).

import { type FlagsService, MODE_HOME, type ModeService } from '@loams/console-host';
import { Slot, useSlot } from '@loams/slots';
import {
  Menu,
  MenuCheckboxItem,
  Sheet,
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
} from '@loams/ui';
import { ArrowLeft, BookOpen, Pin, Settings, SquarePen } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Brand, runShellCommand, SIDEBAR } from './frame.js';
import { iconFor } from './icons.js';
import { useMode } from './mode-switcher.js';
import { useLocation } from './nav.js';
import {
  currentProject,
  isSettingsPage,
  PAGES_CHANGED_EVENT,
  type PageEntry,
  pageAt,
  pageEntries,
  pinnedPages,
  readPinned,
  settingsHome,
  togglePinned,
} from './pages.js';
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
  // `display: contents`: the id the top bar's toggle controls, without a box of its own.
  return (
    <div id="lc-side" className="contents">
      <Sidebar
        label="Sidebar"
        width={width}
        onWidthChange={onWidth}
        min={SIDEBAR.min}
        max={SIDEBAR.max}
        open={open}
        className="lc-side"
      >
        {body}
      </Sidebar>
    </div>
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
  const settings = here !== undefined && isSettingsPage(here);
  if (!settings) back.current = location.path;
  return settings ? (
    <SettingsNav pages={all.filter(isSettingsPage)} current={here.id} back={back.current} />
  ) : (
    <PagesSection all={all} project={project} current={here?.id} />
  );
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
      >
        <Icon aria-hidden="true" size={15} strokeWidth={1.75} />
        <span className="lc-page-label">{page.label}</span>
      </a>
    </li>
  );
}

/** The pins of the open project, live across the sidebar and the palette. */
function usePinned(project: string | undefined): string[] {
  const [pinned, setPinned] = useState(() => readPinned(project));
  useEffect(() => {
    setPinned(readPinned(project));
    const onChange = () => setPinned(readPinned(project));
    window.addEventListener(PAGES_CHANGED_EVENT, onChange);
    return () => window.removeEventListener(PAGES_CHANGED_EVENT, onChange);
  }, [project]);
  return pinned;
}

function PagesSection({
  all,
  project,
  current,
}: {
  all: PageEntry[];
  project: string | undefined;
  current?: string;
}) {
  const pinned = usePinned(project);
  const shown = pinnedPages(all, pinned);
  const pinnable = all.filter((p) => !isSettingsPage(p));
  return (
    <section className="lc-pages" aria-labelledby="lc-pages-title">
      <div className="lc-pages-head">
        <h2 id="lc-pages-title">Pages</h2>
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
      </div>
      {shown.length > 0 ? (
        <ul aria-labelledby="lc-pages-title">
          {shown.map((p) => (
            <PageLink key={p.id} page={p} current={p.id === current} />
          ))}
        </ul>
      ) : (
        <p className="lc-pages-empty">No pinned pages. Pin one here or from the palette.</p>
      )}
    </section>
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
    <nav aria-label="Settings" className="lc-settings-nav">
      <a className="lc-page-link" href={`#${back}`}>
        <ArrowLeft size={15} strokeWidth={1.75} aria-hidden="true" />
        <span className="lc-page-label">Back</span>
      </a>
      <h2 className="lc-pages-head">Settings</h2>
      <ul>
        {pages.map((p) => (
          <PageLink key={p.id} page={p} current={p.id === current} />
        ))}
      </ul>
    </nav>
  );
}

/** The foot's way into Settings (and ⌘, from Task 12). */
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
