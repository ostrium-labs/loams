// Preferences (T2-5, D1080): the `console.settings.section` entries on an edition without
// the desktop's Settings page, so Appearance has a Settings place on the web too. One
// column of sections, each with its own heading; the desktop's Settings page lists the
// same entries, so the page model leaves Preferences out there (pages.ts).

import { useSlot } from '@loams/slots';
import { Empty } from '@loams/ui';

export function PreferencesPage() {
  const sections = useSlot('console.settings.section').filter((e) => e.meta?.id && e.meta.label);
  return (
    <div className="lc-page">
      <header className="lc-page-head">
        <h1>Preferences</h1>
        <p>How the console looks and behaves on this device.</p>
      </header>
      {sections.length === 0 ? (
        <Empty title="No preferences">No plugin offers a preference here.</Empty>
      ) : (
        <div className="lc-preferences">
          {sections.map((e) => {
            const Section = e.component;
            return (
              <div key={e.id} data-settings-section={e.meta?.id}>
                <Section />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
