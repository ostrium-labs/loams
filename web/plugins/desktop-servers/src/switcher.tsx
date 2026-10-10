import type { EngineState, LoamsDesktopApi, ServerEntry } from '@loams/desktop/contracts';
import { ChevronDown } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { PHASE_LABEL, useEngine, useServers } from './state.js';

type Dot = { tone: 'ok' | 'warn' | 'bad' | 'idle'; title: string };

function dotFor(server: ServerEntry | undefined, engine: EngineState | undefined): Dot {
  if (server?.kind !== 'local') return { tone: 'ok', title: 'Connected' };
  if (!engine) return { tone: 'idle', title: 'Checking' };
  const tone = { ready: 'ok', starting: 'warn', failed: 'bad', stopped: 'idle' } as const;
  return { tone: tone[engine.phase], title: PHASE_LABEL[engine.phase] };
}

/** The header server switcher (`shell.header.server`). */
export function Switcher({
  desktop,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  navigate(path: string): void;
}) {
  const { servers, activeId, reload } = useServers(desktop);
  const engine = useEngine(desktop);
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', esc);
    return () => {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', esc);
    };
  }, [open]);

  const active = servers.find((s) => s.id === activeId);
  if (!active) return null;
  const dot = dotFor(active, engine);
  return (
    <div className="lc-server-switch" ref={root}>
      <button
        type="button"
        className="lc-icon-btn"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span
          className={`lc-dot lc-dot-${dot.tone}`}
          title={dot.title}
          role="img"
          aria-label={dot.title}
        />
        <span>{active.name}</span>
        <ChevronDown aria-hidden="true" size={14} />
      </button>
      {open && (
        <div className="lc-server-menu" role="menu" data-overlay-open="">
          {servers.map((s) => (
            <button
              key={s.id}
              type="button"
              role="menuitem"
              aria-current={s.id === activeId ? 'true' : undefined}
              onClick={() => {
                setOpen(false);
                if (s.id !== activeId) void desktop.servers.activate(s.id).then(reload);
              }}
            >
              <span>{s.name}</span>
              <code>{s.url}</code>
            </button>
          ))}
          <hr />
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              navigate('/settings/servers');
            }}
          >
            Manage servers…
          </button>
        </div>
      )}
    </div>
  );
}
