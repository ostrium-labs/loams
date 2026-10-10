// Whether the connected server serves Loams Graph (§48 §18.2). `GetInstance.services[]`
// lists every package of the API catalogue with `available`, so the page can tell a
// server that has never heard of graph from one whose build leaves it out (R0.20).
// `api_versions`, which the console's `flags` read, lists served packages only.

import type { instance } from '@loams/proto';

export const GRAPH_PACKAGE = 'loams.graph.v1';

export type GraphAvailability = 'absent' | 'not_in_variant' | 'available';

export function graphAvailability(
  info: Pick<instance.GetInstanceResponse, 'services'>,
): GraphAvailability {
  const row = info.services.find((s) => s.package === GRAPH_PACKAGE);
  if (!row) return 'absent';
  return row.available ? 'available' : 'not_in_variant';
}
