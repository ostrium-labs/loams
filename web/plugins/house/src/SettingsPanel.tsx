// The settings panel: name and value pairs sent as `ExecuteQuery.settings`. Names come
// from the House's allowlist (settings.ts); a name off the list, on the deny list or a
// value over a cap is flagged here, before the run, with the reason the server gives.

import { Button, Input } from '@loams/ui';
import { useId } from 'react';
import { OFFERED_SETTINGS, type SettingRow, settingProblem } from './settings.js';

export function SettingsPanel({
  rows,
  onChange,
}: {
  rows: SettingRow[];
  onChange: (rows: SettingRow[]) => void;
}) {
  const id = useId();
  const listId = `${id}-names`;
  const nextId = () => rows.reduce((m, r) => Math.max(m, r.id), 0) + 1;
  const update = (rid: number, patch: Partial<SettingRow>) =>
    onChange(rows.map((r) => (r.id === rid ? { ...r, ...patch } : r)));

  return (
    <section aria-labelledby={`${id}-title`} className="flex flex-col gap-2">
      <h3 id={`${id}-title`} className="m-0 text-body">
        Settings
      </h3>
      <p className="m-0 text-small text-muted">
        Only settings the House allows: limits, formats, the analyzer switches and Loams' own
        settings. Others are refused by the server.
      </p>
      <datalist id={listId}>
        {OFFERED_SETTINGS.map((s) => (
          <option key={s} value={s} />
        ))}
      </datalist>
      {rows.length === 0 && <p className="m-0 text-body text-muted">No settings.</p>}
      <ul className="m-0 flex list-none flex-col gap-2 p-0">
        {rows.map((r, i) => {
          const problem =
            r.name.trim() || r.value.trim() ? settingProblem(r.name, r.value) : undefined;
          const errId = `${id}-err-${r.id}`;
          return (
            <li key={r.id} className="flex flex-col gap-1">
              <div className="flex items-center gap-2">
                <Input
                  aria-label={`Setting ${i + 1} name`}
                  list={listId}
                  className={/* mono-ok: id */ 'min-w-0 flex-1 font-mono text-[13px]'}
                  value={r.name}
                  spellCheck={false}
                  autoComplete="off"
                  aria-invalid={problem ? true : undefined}
                  aria-describedby={problem ? errId : undefined}
                  onChange={(e) => update(r.id, { name: e.target.value })}
                />
                <Input
                  aria-label={`Setting ${i + 1} value`}
                  className={/* mono-ok: code */ 'w-28 font-mono text-[13px]'}
                  value={r.value}
                  spellCheck={false}
                  autoComplete="off"
                  aria-invalid={problem ? true : undefined}
                  aria-describedby={problem ? errId : undefined}
                  onChange={(e) => update(r.id, { value: e.target.value })}
                />
                <Button
                  size="sm"
                  variant="quiet"
                  aria-label={`Remove setting ${r.name || i + 1}`}
                  onClick={() => onChange(rows.filter((x) => x.id !== r.id))}
                >
                  Remove
                </Button>
              </div>
              {problem && (
                <p id={errId} className="m-0 text-small text-danger">
                  {problem}
                </p>
              )}
            </li>
          );
        })}
      </ul>
      <div>
        <Button
          size="sm"
          onClick={() => onChange([...rows, { id: nextId(), name: '', value: '' }])}
        >
          Add setting
        </Button>
      </div>
    </section>
  );
}
