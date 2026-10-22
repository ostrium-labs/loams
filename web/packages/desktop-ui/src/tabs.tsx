import { SelectionGroup, SelectionIndicator } from '@loams/ui';
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
      <div role="tablist" className="ds-tabs">
        <SelectionGroup>
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
                className="ds-tab"
                onClick={() => setActive(t.id)}
              >
                {t.label}
                {on && <SelectionIndicator layoutId="page-tab" className="ds-tab-indicator" />}
              </button>
            );
          })}
        </SelectionGroup>
      </div>
      <div role="tabpanel" id={`${base}-panel`} aria-labelledby={`${base}-${current?.id}`}>
        {current?.content}
      </div>
    </div>
  );
}
