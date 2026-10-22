// Settings › Studio › Keyboard shortcuts (SD1 Task 24; §53 §5.10; inventory (h),
// T3 Code's KeybindingsSettings.tsx adapted): every binding with its command, chord,
// `when` clause, source and conflicts. A chord is recorded by pressing it; a `when`
// clause is checked as it is typed and saved on Enter or when the field loses focus.
// Changes are this device's, under `loams.studio.keybindings`.

import {
  buildKeybindingRows,
  type CommandBinding,
  commandLabel,
  groupKeybindingRows,
  type KeybindingRow,
  type KeybindingStore,
  keybindingFromKeyboardEvent,
  parseKeybindingShortcut,
  parseWhenExpressionDraft,
  type ResolvedKeybindingRule,
  shortcutKeys,
  shortcutToKeybindingInput,
  unknownWhenVariables,
  whenAstToExpression,
} from '@loams/studio-ui';
import { Badge, Button, Card, Input, Kbd, KbdGroup } from '@loams/ui';
import { type KeyboardEvent, useState, useSyncExternalStore } from 'react';

function toBinding(rule: ResolvedKeybindingRule): CommandBinding {
  const when = whenAstToExpression(rule.whenAst);
  return { key: shortcutToKeybindingInput(rule.shortcut), ...(when ? { when } : {}) };
}

/** A command's bindings with one of them replaced. */
function replaced(
  store: KeybindingStore,
  row: KeybindingRow,
  next: CommandBinding,
): CommandBinding[] {
  return store
    .resolved()
    .filter((r) => r.command === row.command)
    .map((r) => (r === row.binding ? next : toBinding(r)));
}

function Chord({ value, platform }: { value: string; platform: string }) {
  const shortcut = parseKeybindingShortcut(value);
  if (!shortcut) return <span className="text-muted">{value}</span>;
  return (
    <KbdGroup>
      {shortcutKeys(shortcut, platform).map((k) => (
        <Kbd key={k}>{k}</Kbd>
      ))}
    </KbdGroup>
  );
}

function KeyField({
  row,
  platform,
  onChange,
}: {
  row: KeybindingRow;
  platform: string;
  onChange: (key: string) => void;
}) {
  const [recording, setRecording] = useState(false);
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (!recording) return;
    // The chord is the binding's, not the console's: no palette, no Studio command.
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') {
      setRecording(false);
      return;
    }
    const key = keybindingFromKeyboardEvent(e.nativeEvent, platform);
    if (!key) return;
    setRecording(false);
    onChange(key);
  };
  return (
    <button
      type="button"
      className={
        'inline-flex h-8 min-w-[140px] cursor-pointer items-center justify-start gap-2 rounded-sm border ' +
        'border-rule bg-bg px-2 text-[0.82rem] text-ink hover:border-ink aria-pressed:border-ink'
      }
      aria-pressed={recording}
      aria-label={
        recording
          ? `Press the new shortcut for ${commandLabel(row.command)}, or Escape`
          : `Shortcut for ${commandLabel(row.command)}: ${row.key}. Change`
      }
      onClick={() => setRecording((r) => !r)}
      onBlur={() => setRecording(false)}
      onKeyDown={onKeyDown}
    >
      {recording ? (
        <span className="text-muted">Press keys…</span>
      ) : (
        <Chord value={row.key} platform={platform} />
      )}
    </button>
  );
}

function WhenField({ row, onChange }: { row: KeybindingRow; onChange: (when: string) => void }) {
  const [draft, setDraft] = useState(row.when);
  const parsed = parseWhenExpressionDraft(draft);
  const unknown = parsed.ok ? unknownWhenVariables(parsed.value) : [];
  const commit = () => {
    if (parsed.ok && draft.trim() !== row.when) onChange(draft.trim());
  };
  const id = `when-${row.id.replace(/[^A-Za-z0-9_-]/g, '_')}`;
  return (
    <div className="grid min-w-0 gap-1">
      <Input
        aria-label={`When ${commandLabel(row.command)} applies`}
        aria-invalid={!parsed.ok}
        aria-describedby={!parsed.ok || unknown.length ? id : undefined}
        className={/* mono-ok: code */ 'h-8 font-mono text-[0.78rem]'}
        value={draft}
        placeholder="Always"
        maxLength={256}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit();
          if (e.key === 'Escape') setDraft(row.when);
        }}
      />
      {!parsed.ok && (
        <span id={id} className="text-[0.76rem] text-danger">
          {parsed.message}
        </span>
      )}
      {parsed.ok && unknown.length > 0 && (
        <span id={id} className="text-[0.76rem] text-muted">
          Unknown {unknown.length === 1 ? 'variable' : 'variables'}: {unknown.join(', ')} (always
          false)
        </span>
      )}
    </div>
  );
}

export function KeybindingsCard({ store, platform }: { store: KeybindingStore; platform: string }) {
  const resolved = useSyncExternalStore(store.subscribe, store.resolved);
  const [query, setQuery] = useState('');
  const groups = groupKeybindingRows(buildKeybindingRows(resolved, query));
  const custom = store.rules().length > 0;
  return (
    <Card
      title="Keyboard shortcuts"
      actions={
        custom ? (
          <Button size="sm" variant="quiet" onClick={() => store.resetAll()}>
            Reset all
          </Button>
        ) : undefined
      }
    >
      <div className="flex flex-col gap-4">
        <p className="m-0 text-[0.86rem] text-muted">
          Studio's shortcuts on this device. The console keeps its own in both modes: Ctrl/Cmd+K or
          Ctrl/Cmd+Shift+P (the palette), N (a new thread), B, I and J (the sidebar, inspector and
          drawer), 1–9 and Ctrl+Tab (threads), [ and ] (back and forward), Shift+T (reopen a
          thread), comma (Settings) and Shift+M (the mode).
        </p>
        <Input
          type="search"
          aria-label="Filter shortcuts"
          placeholder="Filter by command, key or condition"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {groups.length === 0 && <p className="m-0 text-muted">No shortcut matches.</p>}
        {groups.map((group) => (
          <section key={group.id} aria-labelledby={`kb-${group.id}`} className="grid gap-1">
            <h3
              id={`kb-${group.id}`}
              className="m-0 text-[0.74rem] font-[540] uppercase tracking-wide text-faint"
            >
              {group.title}
            </h3>
            <ul className="m-0 grid list-none gap-0 p-0">
              {group.rows.map((row) => (
                <li
                  key={row.id}
                  className={
                    'grid items-start gap-x-3 gap-y-1.5 border-b border-rule-soft py-2.5 ' +
                    'sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]'
                  }
                >
                  <div className="grid min-w-0 gap-0.5">
                    <span className="text-[0.88rem] text-ink">{commandLabel(row.command)}</span>
                    {row.conflicts.length > 0 && (
                      <span className="text-[0.76rem] text-danger" role="note">
                        Conflicts with {row.conflicts.join(', ')}
                      </span>
                    )}
                  </div>
                  <KeyField
                    row={row}
                    platform={platform}
                    onChange={(key) =>
                      store.setCommandRules(
                        row.command,
                        replaced(store, row, { key, ...(row.when ? { when: row.when } : {}) }),
                      )
                    }
                  />
                  <WhenField
                    key={`${row.id}:${row.when}`}
                    row={row}
                    onChange={(when) =>
                      store.setCommandRules(
                        row.command,
                        replaced(store, row, { key: row.key, ...(when ? { when } : {}) }),
                      )
                    }
                  />
                  <div className="flex items-center gap-2">
                    <Badge>{row.source}</Badge>
                    {row.source === 'Custom' && (
                      <Button
                        size="sm"
                        variant="quiet"
                        aria-label={`Reset ${commandLabel(row.command)}`}
                        onClick={() => store.resetCommand(row.command)}
                      >
                        Reset
                      </Button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </Card>
  );
}
