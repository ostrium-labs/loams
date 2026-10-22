// The right inspector and the bottom drawer (design 59, D1078). Tabs come from
// `shell.inspector.tab` / `shell.drawer.tab`; a tab with `meta.routes` shows only there. A tab
// mounts the first time it is shown and then stays mounted (hidden), so its state survives
// switching tabs and closing the panel (the agent's chat did the same in the old dock).

import type { PanelService } from '@loams/console-host';
import { SlotBoundary, type SlotEntry, useSlot } from '@loams/slots';
import { IconButton, motion, ResizeHandle, Sheet, Tabs } from '@loams/ui';
import { PanelBottom, PanelRight } from 'lucide-react';
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { ShortcutHint, useNarrow } from './frame.js';
import { useLocation } from './nav.js';
import { DRAWER, INSPECTOR } from './panel.js';
import { usePanelToggle } from './panel-motion.js';
import { compile, type HashRouter } from './router.js';

type TabSlot = 'shell.inspector.tab' | 'shell.drawer.tab';
type Kind = 'inspector' | 'drawer';

const NO_PROPS: Readonly<Record<string, unknown>> = Object.freeze({});

export function tabShows(entry: Pick<SlotEntry, 'meta'>, path: string): boolean {
  const routes = entry.meta?.routes;
  if (!routes || routes.length === 0) return true;
  return routes.some((r) => compile(r).pattern.test(path));
}

function usePanel(panel: PanelService) {
  return useSyncExternalStore(panel.subscribe, () => panel.state());
}

const tabId = (t: SlotEntry<TabSlot>) => t.meta?.id ?? String(t.id);

function useTabs(slot: TabSlot, router: HashRouter) {
  const entries = useSlot(slot);
  // The old right dock (`shell.dock.right`) shows as an inspector tab (W10).
  const dock = useSlot('shell.dock.right');
  const location = useLocation(router);
  const all: SlotEntry<TabSlot>[] =
    slot === 'shell.inspector.tab'
      ? [
          ...entries,
          ...dock.map(
            (d) =>
              ({
                ...d,
                meta: { id: 'dock', label: d.meta?.label ?? 'Panel' },
              }) as unknown as SlotEntry<TabSlot>,
          ),
        ]
      : entries;
  return { tabs: all.filter((e) => tabShows(e, location.path)), params: location.params };
}

const LABEL: Record<Kind, string> = { inspector: 'Inspector', drawer: 'Drawer' };

export function PanelRegion({
  kind,
  panel,
  router,
}: {
  kind: Kind;
  panel: PanelService;
  router: HashRouter;
}) {
  const config = kind === 'inspector' ? INSPECTOR : DRAWER;
  const label = LABEL[kind];
  const state = usePanel(panel);
  const toggle = usePanelToggle(state.open);
  const narrow = useNarrow();
  const { tabs, params } = useTabs(
    kind === 'inspector' ? 'shell.inspector.tab' : 'shell.drawer.tab',
    router,
  );
  const active = tabs.find((t) => tabId(t) === state.tab) ?? tabs[0];
  const activeId = active ? tabId(active) : '';
  const [mounted, setMounted] = useState<ReadonlySet<string>>(() => new Set());
  const region = useRef<HTMLElement>(null);
  const wasOpen = useRef(state.open);
  if (state.open && active && !mounted.has(activeId)) setMounted(new Set([...mounted, activeId]));

  // Focus follows a change, not the first paint: into the tab's marked field (the agent's
  // composer) or the region when it opens, back to its toggle when it closes from inside.
  useEffect(() => {
    if (state.open === wasOpen.current) return;
    wasOpen.current = state.open;
    const el = region.current;
    if (state.open) {
      (
        el?.querySelector<HTMLElement>('[data-panel-autofocus], [data-dock-autofocus]') ?? el
      )?.focus();
    } else if (el?.contains(document.activeElement) || document.activeElement === document.body) {
      // A browser drops focus to the body when its element hides, so both mean "from inside".
      document.querySelector<HTMLElement>(`[data-panel-toggle="${kind}"]`)?.focus();
    }
  }, [state.open, kind]);

  if (tabs.length === 0 || mounted.size === 0) return null;
  const body = (
    <>
      {/* The strip shows with one tab too: it names what the panel holds. */}
      <div className="lc-panel-tabs">
        <Tabs
          label={`${label} tabs`}
          value={activeId}
          onValueChange={(tab) => panel.open({ tab })}
          tabs={tabs.map((t) => ({ value: tabId(t), label: t.meta?.label ?? 'Tab' }))}
        />
      </div>
      {tabs.map((t) => {
        const id = tabId(t);
        if (!mounted.has(id)) return null;
        const Entry = t.component;
        return (
          <div
            key={t.id}
            className="lc-panel-tab"
            role="tabpanel"
            aria-label={t.meta?.label ?? 'Tab'}
            hidden={id !== activeId}
          >
            <SlotBoundary plugin={t.plugin}>
              <Entry params={params} props={id === state.tab ? state.props : NO_PROPS} />
            </SlotBoundary>
          </div>
        );
      })}
    </>
  );
  // Below 960px the inspector is an overlay from the right (D1078).
  if (kind === 'inspector' && narrow) {
    return (
      <Sheet open={state.open} onClose={() => panel.close()} title={label} side="right">
        <div className="lc-inspector-sheet">{body}</div>
      </Sheet>
    );
  }
  const id = `lc-${kind}`;
  const handle = state.open && (
    <ResizeHandle
      label={`Resize ${kind}`}
      value={state.size}
      min={config.min}
      max={config.max}
      orientation={kind === 'inspector' ? 'vertical' : 'horizontal'}
      edge="start"
      controls={id}
      className="lc-panel-handle"
      onResize={(n) => panel.resize(n)}
    />
  );
  // The clip carries the panel's size, so with Motion on a toggle slides it on the panel
  // curve (UI1-1, UI2); during that slide the panel holds its size and the clip cuts it.
  const inspector = kind === 'inspector';
  const open = state.open;
  const sized = toggle.animating ? state.size : '100%';
  return (
    <>
      {handle}
      <motion.div
        className={inspector ? 'lc-panel-clip lc-inspector-clip' : 'lc-panel-clip lc-drawer-clip'}
        // The first opening mounts the clip: with Motion on it starts shut, so it slides too.
        initial={toggle.animating ? (inspector ? { width: 0 } : { height: 0 }) : false}
        animate={inspector ? { width: open ? state.size : 0 } : { height: open ? state.size : 0 }}
        transition={toggle.transition}
        onAnimationComplete={toggle.done}
      >
        {inspector ? (
          <aside
            ref={region}
            id={id}
            className="lc-inspector"
            aria-label={label}
            tabIndex={-1}
            hidden={!toggle.shown}
            inert={toggle.closing || undefined}
            style={{ width: sized }}
          >
            {body}
          </aside>
        ) : (
          <section
            ref={region}
            id={id}
            className="lc-drawer"
            aria-label={label}
            tabIndex={-1}
            hidden={!toggle.shown}
            inert={toggle.closing || undefined}
            style={{ height: sized }}
          >
            {body}
          </section>
        )}
      </motion.div>
    </>
  );
}

/** The top bar's inspector and drawer toggles; each shows only while it has a tab here. */
export function PanelToggles({
  inspector,
  drawer,
  router,
}: {
  inspector: PanelService;
  drawer: PanelService;
  router: HashRouter;
}) {
  const i = usePanel(inspector);
  const d = usePanel(drawer);
  const inspectorTabs = useTabs('shell.inspector.tab', router).tabs.length;
  const drawerTabs = useTabs('shell.drawer.tab', router).tabs.length;
  return (
    <>
      {drawerTabs > 0 && (
        <IconButton
          size="sm"
          className="lc-bar-btn"
          data-panel-toggle="drawer"
          aria-pressed={d.open}
          aria-controls="lc-drawer"
          label={d.open ? 'Hide drawer' : 'Show drawer'}
          tooltip={<ShortcutHint label={d.open ? 'Hide drawer' : 'Show drawer'} keys="J" />}
          aria-keyshortcuts="Control+J Meta+J"
          onClick={() => drawer.toggle()}
        >
          <PanelBottom strokeWidth={1.75} aria-hidden="true" />
        </IconButton>
      )}
      {inspectorTabs > 0 && (
        <IconButton
          size="sm"
          className="lc-bar-btn"
          data-panel-toggle="inspector"
          aria-pressed={i.open}
          aria-controls="lc-inspector"
          label={i.open ? 'Hide inspector' : 'Show inspector'}
          tooltip={<ShortcutHint label={i.open ? 'Hide inspector' : 'Show inspector'} keys="I" />}
          aria-keyshortcuts="Control+I Meta+I"
          onClick={() => inspector.toggle()}
        >
          <PanelRight strokeWidth={1.75} aria-hidden="true" />
        </IconButton>
      )}
    </>
  );
}
