// `/metrics`: dashboards of live metric panels. Each dashboard is a @loams-core/dashboards
// spec laid out on react-grid-layout (as dashboard-ui lays it out); each panel is a
// @loams/charts LiveChart or a stat tile. Edits stay in a draft per dashboard until Save.

import { Button, Empty, Field, Notice, SampleDataNotice, Select } from '@loams/ui';
import type { DashboardSpec, Widget } from '@loams-core/types';
import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { type Layout, Responsive, WidthProvider } from 'react-grid-layout/legacy';
import 'react-grid-layout/css/styles.css';
import 'react-resizable/css/styles.css';
import { type MetricGroup, metricDef } from './catalog.js';
import { failureText, type MetricsClient, VersionConflict } from './client.js';
import { DEFAULT_DASHBOARDS } from './defaults.js';
import { type Nudge, Panel } from './Panel.js';
import { type PanelChoice, PanelDialog } from './PanelDialog.js';
import {
  addPanel,
  applyGrid,
  GRID_COLUMNS,
  isDirty,
  MIN_H,
  MIN_W,
  newPanelId,
  nudgePanel,
  panelsOf,
  panelWidget,
  REFRESH_RATES,
  rangeOf,
  refreshOf,
  removePanel,
  setParam,
  stepFor,
  TIME_RANGES,
  updatePanel,
  validateDashboard,
} from './spec.js';

const ResponsiveGrid = WidthProvider(Responsive);
const BREAKPOINTS = { lg: 1100, md: 800, sm: 560, xs: 0 };
const COLS = { lg: GRID_COLUMNS, md: GRID_COLUMNS, sm: 6, xs: 1 };
const ROW_HEIGHT = 92;

type Loaded =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; saved: DashboardSpec[] };

type DialogState = { mode: 'add' } | { mode: 'edit'; id: string } | null;

function groupOf(spec: DashboardSpec): MetricGroup | undefined {
  const first = panelsOf(spec)[0];
  return first ? metricDef(first.metric)?.group : undefined;
}

export function MetricsPage({ client, sample }: { client: MetricsClient; sample?: boolean }) {
  const [loaded, setLoaded] = useState<Loaded>({ phase: 'loading' });
  const [reload, setReload] = useState(0);
  const [selected, setSelected] = useState<string>(DEFAULT_DASHBOARDS[0]?.id ?? '');
  const [drafts, setDrafts] = useState<Record<string, DashboardSpec>>({});
  const [editing, setEditing] = useState(false);
  const [dialog, setDialog] = useState<DialogState>(null);
  const [dialogKey, setDialogKey] = useState(0);
  const [tick, setTick] = useState(0);
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<{ title: string; detail: string; conflict?: boolean }>();
  const [announcement, setAnnouncement] = useState('');
  const [cols, setCols] = useState(GRID_COLUMNS);
  const tabRefs = useRef(new Map<string, HTMLButtonElement>());
  // A draft whose save hit a version conflict: dropped once the saved version has loaded,
  // and kept if that load fails, so a failed reload never loses the edits.
  const staleDraft = useRef<string | undefined>(undefined);

  // oxlint-disable-next-line react/exhaustive-deps -- `reload` re-reads on demand
  useEffect(() => {
    const ctl = new AbortController();
    setLoaded({ phase: 'loading' });
    client
      .listDashboards(ctl.signal)
      .then((saved) => {
        if (ctl.signal.aborted) return;
        setLoaded({ phase: 'ready', saved });
        const stale = staleDraft.current;
        staleDraft.current = undefined;
        if (stale) setDrafts(({ [stale]: _stale, ...rest }) => rest);
        setSelected((s) => (saved.some((d) => d.id === s) ? s : (saved[0]?.id ?? '')));
      })
      .catch((e) => !ctl.signal.aborted && setLoaded({ phase: 'error', message: failureText(e) }));
    return () => ctl.abort();
  }, [client, reload]);

  const saved = loaded.phase === 'ready' ? loaded.saved : [];
  const base = saved.find((d) => d.id === selected);
  const spec = (base && drafts[base.id]) ?? base;
  const dirty = Boolean(base && spec && isDirty(base, spec));
  const panels = useMemo(() => (spec ? panelsOf(spec) : []), [spec]);
  const range = spec ? rangeOf(spec) : undefined;
  const refresh = spec ? refreshOf(spec) : undefined;

  const edit = useCallback(
    (next: (s: DashboardSpec) => DashboardSpec, say?: string) => {
      if (!spec) return;
      const updated = next(spec);
      if (updated === spec) return;
      setDrafts((d) => ({ ...d, [updated.id]: updated }));
      if (say) setAnnouncement(say);
    },
    [spec],
  );

  const onGridStop = useCallback(
    (layout: Layout) => {
      // A narrow breakpoint reflows the grid into fewer columns; only a 12-column layout is
      // the dashboard's own, so only that one is written back.
      if (cols !== GRID_COLUMNS) return;
      edit((s) => applyGrid(s, layout));
    },
    [cols, edit],
  );

  const nudge = (id: string, title: string, d: Nudge) => {
    if (!spec) return;
    // Announce where the panel ended up after clamping, not where the key asked for.
    const was = spec.layout.find((l) => l.id === id);
    const next = nudgePanel(spec, id, d);
    const it = next.layout.find((l) => l.id === id);
    if (!was || !it) return;
    const moved = it.x !== was.x || it.y !== was.y;
    edit(() => next);
    setAnnouncement(
      d.dx || d.dy
        ? `${title} ${moved ? 'moved to' : 'stays at'} column ${it.x + 1}, row ${it.y + 1}.`
        : `${title} is ${it.w} columns by ${it.h} rows.`,
    );
  };

  const remove = (id: string, title: string) => {
    edit((s) => removePanel(s, id), `${title} removed.`);
    // Focus would be lost with the panel; hand it to the toolbar.
    requestAnimationFrame(() => document.getElementById('mx-add')?.focus());
  };

  const submitPanel = (choice: PanelChoice) => {
    if (!spec || !dialog) return;
    if (dialog.mode === 'add') {
      const id = newPanelId(spec, choice.metric);
      edit((s) => addPanel(s, panelWidget(id, choice.metric, choice.type, choice.title)));
      setAnnouncement(`${choice.title} added at the bottom.`);
      requestAnimationFrame(() =>
        (document.querySelector(`[data-panel="${CSS.escape(id)}"]`) as HTMLElement | null)?.focus(),
      );
    } else {
      edit((s) => updatePanel(s, dialog.id, choice), `${choice.title} updated.`);
    }
    setDialog(null);
  };

  const save = async () => {
    if (!base || !spec) return;
    const v = validateDashboard(spec);
    if (!v.valid) {
      setProblem({ title: 'This dashboard cannot be saved', detail: v.errors.join('; ') });
      return;
    }
    setSaving(true);
    setProblem(undefined);
    try {
      const next = await client.saveDashboard(v.spec, base.version);
      setLoaded((l) =>
        l.phase === 'ready'
          ? { phase: 'ready', saved: l.saved.map((d) => (d.id === next.id ? next : d)) }
          : l,
      );
      setDrafts(({ [next.id]: _done, ...rest }) => rest);
      setAnnouncement(`${next.title} saved.`);
    } catch (e) {
      setProblem({
        title: 'Not saved',
        detail: failureText(e),
        conflict: e instanceof VersionConflict,
      });
    } finally {
      setSaving(false);
    }
  };

  const discard = () => {
    if (!base) return;
    setDrafts(({ [base.id]: _gone, ...rest }) => rest);
    setProblem(undefined);
    setAnnouncement('Changes discarded.');
  };

  const resetToDefault = () => {
    const def = DEFAULT_DASHBOARDS.find((d) => d.id === base?.id);
    if (!def || !base) return;
    setDrafts((d) => ({ ...d, [base.id]: { ...structuredClone(def), version: base.version } }));
    setAnnouncement(`${base.title} reset to its default panels. Save to keep it.`);
  };

  const onTabKey = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const n = saved.length;
    const to =
      e.key === 'ArrowRight'
        ? (index + 1) % n
        : e.key === 'ArrowLeft'
          ? (index - 1 + n) % n
          : e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? n - 1
              : -1;
    if (to < 0) return;
    e.preventDefault();
    const id = saved[to]?.id;
    if (!id) return;
    setSelected(id);
    tabRefs.current.get(id)?.focus();
  };

  if (loaded.phase === 'loading') {
    return (
      <div className="lc-page">
        <p role="status">Loading dashboards…</p>
      </div>
    );
  }
  if (loaded.phase === 'error') {
    return (
      <div className="lc-page">
        <Notice tone="danger" title="Could not load dashboards">
          {loaded.message}{' '}
          <Button size="sm" onClick={() => setReload((r) => r + 1)}>
            Retry
          </Button>
        </Notice>
      </div>
    );
  }

  const gridLayout: Layout = panels.map((p) => ({
    i: p.id,
    x: p.layout.x,
    y: p.layout.y,
    w: p.layout.w,
    h: p.layout.h,
    minW: MIN_W,
    minH: MIN_H,
  }));
  const editingPanel = dialog?.mode === 'edit' ? panels.find((p) => p.id === dialog.id) : undefined;
  const tabPanelId = 'mx-tabpanel';

  return (
    <div className="lc-page">
      <header className="lc-page-head flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1>Metrics</h1>
          <p>Usage, Postgres, Loams SQL, Live and House, as they happen.</p>
        </div>
        <div className="flex flex-wrap items-end gap-2">
          <Field label="Time range">
            {(p) => (
              <Select
                {...p}
                value={range?.id}
                onChange={(e) => edit((s) => setParam(s, 'range', e.target.value))}
              >
                {TIME_RANGES.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.label}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Field label="Refresh">
            {(p) => (
              <Select
                {...p}
                value={refresh?.id}
                onChange={(e) => edit((s) => setParam(s, 'refresh', e.target.value))}
              >
                {REFRESH_RATES.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.label}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Button size="sm" onClick={() => setTick((t) => t + 1)}>
            Refresh now
          </Button>
          <Button
            size="sm"
            variant={editing ? 'primary' : 'secondary'}
            aria-pressed={editing}
            onClick={() => setEditing((e) => !e)}
          >
            {editing ? 'Done editing' : 'Edit layout'}
          </Button>
        </div>
      </header>
      {sample && (
        <div className="mb-3">
          <SampleDataNotice>The charts show generated values, not your metrics.</SampleDataNotice>
        </div>
      )}

      <div role="tablist" aria-label="Dashboards" className="mb-3 flex flex-wrap gap-1">
        {saved.map((d, i) => {
          const isSel = d.id === selected;
          const changed = drafts[d.id] && isDirty(d, drafts[d.id] as DashboardSpec);
          return (
            <button
              key={d.id}
              ref={(el) => {
                if (el) tabRefs.current.set(d.id, el);
                else tabRefs.current.delete(d.id);
              }}
              type="button"
              role="tab"
              id={`mx-tab-${d.id}`}
              aria-selected={isSel}
              aria-controls={tabPanelId}
              tabIndex={isSel ? 0 : -1}
              onClick={() => setSelected(d.id)}
              onKeyDown={(e) => onTabKey(e, i)}
              className={
                isSel
                  ? 'cursor-pointer rounded-pill border-0 bg-accent-soft px-3 py-1 text-sm text-accent-fg'
                  : 'cursor-pointer rounded-pill border-0 bg-transparent px-3 py-1 text-sm text-muted hover:bg-raised hover:text-ink'
              }
            >
              {d.title}
              {changed ? (
                <>
                  <span aria-hidden="true"> •</span>
                  <span className="sr-only"> (unsaved)</span>
                </>
              ) : null}
            </button>
          );
        })}
      </div>

      <div className="sr-only" aria-live="polite" role="status">
        {announcement}
      </div>

      {problem && (
        <Notice tone="danger" title={problem.title}>
          {problem.detail}{' '}
          {problem.conflict && (
            <Button
              size="sm"
              onClick={() => {
                setProblem(undefined);
                staleDraft.current = base?.id;
                setReload((r) => r + 1);
              }}
            >
              Load the saved version
            </Button>
          )}
        </Notice>
      )}

      {(editing || dirty) && (
        <div
          className="mb-3 flex flex-wrap items-center gap-2"
          role="toolbar"
          aria-label="Dashboard edits"
        >
          {editing && (
            <>
              <Button
                id="mx-add"
                size="sm"
                onClick={() => {
                  setDialogKey((k) => k + 1);
                  setDialog({ mode: 'add' });
                }}
              >
                Add panel
              </Button>
              {DEFAULT_DASHBOARDS.some((d) => d.id === base?.id) && (
                <Button size="sm" variant="quiet" onClick={resetToDefault}>
                  Reset to default
                </Button>
              )}
              <span className="text-xs text-muted">
                Drag a panel by its head, or focus it and use the arrow keys to move, Shift+arrows
                to resize, Delete to remove.
              </span>
            </>
          )}
          {dirty && (
            <span className="ml-auto flex gap-2">
              <Button size="sm" variant="quiet" onClick={discard} disabled={saving}>
                Discard
              </Button>
              <Button size="sm" variant="primary" onClick={save} disabled={saving}>
                {saving ? 'Saving…' : 'Save dashboard'}
              </Button>
            </span>
          )}
        </div>
      )}

      <div
        id={tabPanelId}
        role="tabpanel"
        aria-labelledby={spec ? `mx-tab-${spec.id}` : undefined}
        className="[&_.react-grid-placeholder]:bg-accent-soft motion-reduce:[&_.react-grid-item]:transition-none"
      >
        {!spec || panels.length === 0 ? (
          <Empty
            title="No panels"
            actions={
              <Button
                size="sm"
                onClick={() => {
                  setEditing(true);
                  setDialogKey((k) => k + 1);
                  setDialog({ mode: 'add' });
                }}
              >
                Add panel
              </Button>
            }
          >
            Add a panel to chart a metric.
          </Empty>
        ) : (
          <ResponsiveGrid
            layouts={{ lg: gridLayout }}
            breakpoints={BREAKPOINTS}
            cols={COLS}
            rowHeight={ROW_HEIGHT}
            margin={[12, 12]}
            isDraggable={editing && cols === GRID_COLUMNS}
            isResizable={editing && cols === GRID_COLUMNS}
            draggableHandle=".mx-drag"
            draggableCancel="button"
            onDragStop={onGridStop}
            onResizeStop={onGridStop}
            onBreakpointChange={(_bp: string, n: number) => setCols(n)}
          >
            {panels.map((p) => (
              <div key={p.id}>
                <Panel
                  panel={p}
                  widget={spec.widgets[p.id] as Widget}
                  rows={p.layout.h}
                  client={client}
                  rangeMs={range?.ms ?? 0}
                  stepMs={stepFor(range?.ms ?? 0)}
                  intervalMs={refresh?.ms ?? 0}
                  tick={tick}
                  editing={editing}
                  onEdit={() => {
                    setDialogKey((k) => k + 1);
                    setDialog({ mode: 'edit', id: p.id });
                  }}
                  onRemove={() => remove(p.id, p.title)}
                  onNudge={(d) => nudge(p.id, p.title, d)}
                />
              </div>
            ))}
          </ResponsiveGrid>
        )}
      </div>

      {dialog && (
        <PanelDialog
          key={dialogKey}
          open
          editing={editingPanel}
          defaultGroup={spec ? groupOf(spec) : undefined}
          onClose={() => setDialog(null)}
          onSubmit={submitPanel}
        />
      )}
    </div>
  );
}
