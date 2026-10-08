import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { EngineCard } from './engine-card.js';
import { useServers } from './state.js';

/** `environment.overview.card`: the engine card, only while the active server is local. */
export function OverviewEngineCard({ desktop }: { desktop: LoamsDesktopApi }) {
  const { servers, activeId } = useServers(desktop);
  if (servers.find((s) => s.id === activeId)?.kind !== 'local') return null;
  return <EngineCard desktop={desktop} />;
}
