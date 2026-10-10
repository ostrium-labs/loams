import type { LoamsDesktopApi, StackId, StackState } from '@loams/desktop/contracts';
import { useEffect, useState } from 'react';

/** The live state of one local stack; `undefined` until the first answer. */
export function useStack(stacks: LoamsDesktopApi['stacks'], id: StackId): StackState | undefined {
  const [state, setState] = useState<StackState>();
  useEffect(() => {
    let live = true;
    stacks
      .state(id)
      .then((s) => live && setState(s))
      .catch((e) => live && setState({ phase: 'error', message: String(e?.message ?? e) }));
    const off = stacks.onState((sid, s) => {
      if (sid === id && live) setState(s);
    });
    return () => {
      live = false;
      off();
    };
  }, [stacks, id]);
  return state;
}
