// The palette: sources and sinks by connector family, then the transforms. Each entry is
// a button (click or Enter adds the node at the canvas centre) and is draggable onto the
// canvas, so the keyboard path and the pointer path add the same node.

import { SearchInput } from '@loams/ui';
import { type DragEvent, useMemo, useState } from 'react';
import {
  type ConnectorDef,
  type Direction,
  FAMILY_LABEL,
  type Family,
  TRANSFORM_HINT,
  TRANSFORM_LABEL,
  type TransformKind,
} from './model.js';

/** What a palette entry adds; also the drag payload. */
export type PaletteItem =
  | { kind: Direction; ref: string; label: string }
  | { kind: 'transform'; ref: TransformKind; label: string };

export const DRAG_MIME = 'application/x-loams-pipeline-node';

export function encodeItem(item: PaletteItem): string {
  return JSON.stringify(item);
}

export function decodeItem(raw: string): PaletteItem | undefined {
  try {
    const v = JSON.parse(raw) as Partial<PaletteItem>;
    if (
      (v.kind === 'source' || v.kind === 'sink' || v.kind === 'transform') &&
      typeof v.ref === 'string' &&
      typeof v.label === 'string'
    ) {
      return v as PaletteItem;
    }
  } catch {
    // not ours
  }
  return undefined;
}

/** Connectors that serve a direction, grouped by family in the catalog's family order. */
export function byFamily(
  catalog: readonly ConnectorDef[],
  direction: Direction,
  query = '',
): [Family, ConnectorDef[]][] {
  const q = query.trim().toLowerCase();
  const groups = new Map<Family, ConnectorDef[]>();
  for (const c of catalog) {
    if (!c.directions.includes(direction)) continue;
    if (q && !`${c.name} ${c.id} ${FAMILY_LABEL[c.family]}`.toLowerCase().includes(q)) continue;
    const list = groups.get(c.family) ?? [];
    list.push(c);
    groups.set(c.family, list);
  }
  return [...groups.entries()];
}

function Entry({
  item,
  title,
  onAdd,
}: {
  item: PaletteItem;
  title?: string;
  onAdd: (i: PaletteItem) => void;
}) {
  const onDragStart = (e: DragEvent<HTMLButtonElement>) => {
    e.dataTransfer.setData(DRAG_MIME, encodeItem(item));
    e.dataTransfer.effectAllowed = 'copy';
  };
  return (
    <li>
      <button
        type="button"
        draggable
        onDragStart={onDragStart}
        onClick={() => onAdd(item)}
        title={title}
        aria-label={`Add ${item.label} ${item.kind}`}
        className="flex h-7 w-full cursor-grab items-center rounded-sm border border-solid border-rule-soft bg-surface px-2 text-left text-body text-ink transition-colors duration-(--op-duration-fast) hover:border-control-border hover:bg-raised focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus"
      >
        {item.label}
      </button>
    </li>
  );
}

function Section({
  title,
  direction,
  catalog,
  query,
  onAdd,
}: {
  title: string;
  direction: Direction;
  catalog: readonly ConnectorDef[];
  query: string;
  onAdd: (i: PaletteItem) => void;
}) {
  const groups = byFamily(catalog, direction, query);
  return (
    <section aria-label={title} className="flex flex-col gap-2">
      <h3 className="m-0 text-small font-medium text-muted">{title}</h3>
      {groups.length === 0 && <p className="m-0 text-small text-faint">No match.</p>}
      {groups.map(([family, list]) => (
        <div key={family} className="flex flex-col gap-1">
          <h4 className="m-0 text-micro text-faint">{FAMILY_LABEL[family]}</h4>
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {list.map((c) => (
              <Entry
                key={c.id}
                item={{ kind: direction, ref: c.id, label: c.name }}
                onAdd={onAdd}
              />
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}

export function Palette({
  catalog,
  onAdd,
  disabled,
}: {
  catalog: readonly ConnectorDef[];
  onAdd: (item: PaletteItem) => void;
  disabled?: boolean;
}) {
  const [query, setQuery] = useState('');
  const transforms = useMemo(() => Object.keys(TRANSFORM_LABEL) as TransformKind[], []);
  return (
    <nav aria-label="Node palette" className="flex min-w-0 flex-col gap-4 overflow-y-auto">
      <fieldset disabled={disabled} className="m-0 flex flex-col gap-4 border-0 p-0">
        <SearchInput
          size="compact"
          label="Filter connectors"
          placeholder="Filter connectors"
          value={query}
          onValueChange={setQuery}
        />
        <Section title="Sources" direction="source" catalog={catalog} query={query} onAdd={onAdd} />
        <section aria-label="Transforms" className="flex flex-col gap-2">
          <h3 className="m-0 text-small font-medium text-muted">Transforms</h3>
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {transforms.map((t) => (
              <Entry
                key={t}
                item={{ kind: 'transform', ref: t, label: TRANSFORM_LABEL[t] }}
                title={TRANSFORM_HINT[t]}
                onAdd={onAdd}
              />
            ))}
          </ul>
        </section>
        <Section title="Sinks" direction="sink" catalog={catalog} query={query} onAdd={onAdd} />
      </fieldset>
    </nav>
  );
}
