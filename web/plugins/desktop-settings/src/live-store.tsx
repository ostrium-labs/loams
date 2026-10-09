import type { EngineState, LiveStoreChoice, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Card, Field, Notice, Select } from '@loams/ui';
import { useEffect, useState } from 'react';

type EngineEvents = Partial<Pick<LoamsDesktopApi['engine'], 'state' | 'onState'>>;

/** The engine's Live notice (why Live runs on local data although TiKV was chosen), live. */
export function useLiveNotice(engine?: EngineEvents): string | undefined {
  const [notice, setNotice] = useState<string>();
  useEffect(() => {
    if (!engine?.state || !engine.onState) return;
    let live = true;
    const take = (s: EngineState) => {
      if (live) setNotice(s.phase === 'ready' ? s.liveNotice : undefined);
    };
    engine
      .state()
      .then(take)
      .catch(() => undefined);
    const off = engine.onState(take);
    return () => {
      live = false;
      off();
    };
  }, [engine]);
  return notice;
}

/**
 * Settings > Local stacks > Live: where this computer's Live keeps its data (ruling T23-8). The
 * embedded store is the default; the TiKV stack is used only when chosen here and running, never
 * because it happens to run. Hidden on an older desktop bridge without the choice.
 */
export function LiveStoreCard({ desktop }: { desktop: LoamsDesktopApi }) {
  const engine = desktop.engine;
  const [choice, setChoice] = useState<LiveStoreChoice>();
  const [error, setError] = useState<string>();
  const notice = useLiveNotice(engine);
  useEffect(() => {
    if (!engine.liveStore) return;
    let live = true;
    engine
      .liveStore()
      .then((c) => live && setChoice(c))
      .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [engine]);
  if (!engine.liveStore) return null;

  const change = async (next: LiveStoreChoice) => {
    setChoice(next);
    setError(undefined);
    try {
      const res = await engine.setLiveStore(next);
      if (!res.ok) setError(res.message);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <Card title="Live">
      <div className="flex flex-col gap-3">
        <Field
          label="Live store"
          hint="Where Live keeps this computer's data. The TiKV stack is used only when chosen here and running."
        >
          {(p) => (
            <Select
              {...p}
              value={choice ?? 'embedded'}
              disabled={!choice}
              onChange={(e) => void change(e.target.value as LiveStoreChoice)}
            >
              <option value="embedded">Local (embedded store)</option>
              <option value="tikv-stack">TiKV stack</option>
            </Select>
          )}
        </Field>
        {notice && (
          <Notice tone="warn" title="Live on TiKV">
            {notice}
          </Notice>
        )}
        {error && (
          <Notice tone="danger" title="Could not save the Live store">
            {error}
          </Notice>
        )}
      </div>
    </Card>
  );
}
