import { type ReactNode, useId, useState } from 'react';

export interface TabDef {
  id: string;
  label: string;
  content: ReactNode;
}

/** A minimal accessible tab strip; only the active panel is mounted. */
export function Tabs({ tabs, initial }: { tabs: TabDef[]; initial?: string }) {
  const [active, setActive] = useState(initial ?? tabs[0]?.id);
  const base = useId();
  const current = tabs.find((t) => t.id === active) ?? tabs[0];
  return (
    <div className="flex flex-col gap-4">
      <div role="tablist" className="flex gap-1 border-b border-rule">
        {tabs.map((t) => {
          const on = t.id === current?.id;
          return (
            <button
              key={t.id}
              type="button"
              role="tab"
              id={`${base}-${t.id}`}
              aria-selected={on}
              aria-controls={`${base}-panel`}
              className={
                on
                  ? 'cursor-pointer border-0 border-b-2 border-solid border-accent bg-transparent px-4 py-2 font-sans text-sm font-medium text-ink'
                  : 'cursor-pointer border-0 border-b-2 border-solid border-transparent bg-transparent px-4 py-2 font-sans text-sm text-muted'
              }
              onClick={() => setActive(t.id)}
            >
              {t.label}
            </button>
          );
        })}
      </div>
      <div role="tabpanel" id={`${base}-panel`} aria-labelledby={`${base}-${current?.id}`}>
        {current?.content}
      </div>
    </div>
  );
}
