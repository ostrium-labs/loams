import type { PgTimeline } from '@loams/desktop/contracts';

export interface TimelineRow {
  timeline: PgTimeline;
  depth: number;
}

/**
 * Timelines as a tree by ancestor, flattened depth-first for rendering. A
 * timeline whose ancestor is unknown is treated as a root. Siblings sort by
 * name, then id, so the order is stable.
 */
export function flattenTimelines(timelines: PgTimeline[]): TimelineRow[] {
  const ids = new Set(timelines.map((t) => t.timelineId));
  const children = new Map<string, PgTimeline[]>();
  const roots: PgTimeline[] = [];
  for (const t of timelines) {
    if (
      t.ancestorTimelineId &&
      ids.has(t.ancestorTimelineId) &&
      t.ancestorTimelineId !== t.timelineId
    ) {
      const list = children.get(t.ancestorTimelineId) ?? [];
      list.push(t);
      children.set(t.ancestorTimelineId, list);
    } else roots.push(t);
  }
  const cmp = (a: PgTimeline, b: PgTimeline) =>
    (a.name ?? '').localeCompare(b.name ?? '') || a.timelineId.localeCompare(b.timelineId);
  const out: TimelineRow[] = [];
  const seen = new Set<string>();
  const walk = (t: PgTimeline, depth: number) => {
    if (seen.has(t.timelineId)) return;
    seen.add(t.timelineId);
    out.push({ timeline: t, depth });
    for (const c of (children.get(t.timelineId) ?? []).sort(cmp)) walk(c, depth + 1);
  };
  for (const r of roots.sort(cmp)) walk(r, 0);
  return out;
}
