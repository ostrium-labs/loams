import type { EngineState, LoamsDesktopApi, ServerEntry } from '@loams/desktop/contracts';
import { useCallback, useEffect, useState } from 'react';

export interface ServersState {
  servers: ServerEntry[];
  activeId: string;
  loading: boolean;
  error?: string;
  reload(): Promise<void>;
}

/** The registry's servers and which one is active. */
export function useServers(desktop: LoamsDesktopApi): ServersState {
  const [state, setState] = useState<Omit<ServersState, 'reload'>>({
    servers: [],
    activeId: '',
    loading: true,
  });
  const reload = useCallback(async () => {
    try {
      const { servers, activeId } = await desktop.servers.list();
      setState({ servers, activeId, loading: false });
    } catch (e) {
      setState((s) => ({
        ...s,
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      }));
    }
  }, [desktop]);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { ...state, reload };
}

/** The local engine's phase, live. `undefined` until the first state arrives. */
export function useEngine(desktop: LoamsDesktopApi): EngineState | undefined {
  const [engine, setEngine] = useState<EngineState>();
  useEffect(() => {
    let live = true;
    // Subscribe first so a transition between state() and the listener is not lost.
    const off = desktop.engine.onState((s) => live && setEngine(s));
    desktop.engine
      .state()
      .then((s) => live && setEngine((cur) => cur ?? s))
      .catch(() => undefined);
    return () => {
      live = false;
      off();
    };
  }, [desktop]);
  return engine;
}

export const PHASE_LABEL: Record<EngineState['phase'], string> = {
  stopped: 'Stopped',
  starting: 'Starting',
  ready: 'Ready',
  failed: 'Failed',
};

export type Tone = 'done' | 'progress' | 'planned' | 'failed';
export const PHASE_TONE: Record<EngineState['phase'], Tone> = {
  stopped: 'planned',
  starting: 'progress',
  ready: 'done',
  failed: 'failed',
};
