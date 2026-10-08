// The Settings area: a section list on the left and the chosen section on
// the right. Sections come from the `console.settings.section` slot, so a
// plugin adds one by registering `{ meta: { id, label }, order }` (the Agent
// providers section does exactly that); this page knows none of them.

import { type SlotEntry, useSlot } from '@loams/slots';
import { Empty } from '@loams/ui';

export const SETTINGS_PATH = '/settings';

/** Entries that declare an id and a label, in order. */
function usable(entries: SlotEntry<'console.settings.section'>[]) {
  return entries.filter((e) => e.meta?.id && e.meta.label);
}

export function SettingsPage({ section }: { section?: string }) {
  const sections = usable(useSlot('console.settings.section'));
  const current = section ? sections.find((e) => e.meta?.id === section) : sections[0];
  const Section = current?.component;
  return (
    <div className="lc-page">
      <header className="lc-page-head">
        <h1>Settings</h1>
        <p>Servers, local stacks, updates and what this app is.</p>
      </header>
      <div className="flex gap-6 items-start">
        <nav aria-label="Settings sections" className="w-48 shrink-0">
          <ul className="flex flex-col gap-1 list-none m-0 p-0">
            {sections.map((e) => {
              const active = e === current;
              return (
                <li key={e.id}>
                  <a
                    href={`#${SETTINGS_PATH}/${e.meta?.id}`}
                    aria-current={active ? 'page' : undefined}
                    className={
                      active
                        ? 'block px-3 py-2 text-sm font-medium no-underline bg-accent-soft text-ink'
                        : 'block px-3 py-2 text-sm no-underline text-muted'
                    }
                  >
                    {e.meta?.label}
                  </a>
                </li>
              );
            })}
          </ul>
        </nav>
        <div className="min-w-0 flex-1" data-settings-section={current?.meta?.id}>
          {Section ? (
            <Section />
          ) : (
            <Empty title="No such section">
              {section ? (
                <>
                  Settings has no section <code>{section}</code>.
                </>
              ) : (
                'No settings sections are available.'
              )}
            </Empty>
          )}
        </div>
      </div>
    </div>
  );
}
