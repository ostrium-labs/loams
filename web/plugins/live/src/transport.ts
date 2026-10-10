import type { Transport } from '@connectrpc/connect';

/**
 * The one place that decides how the Live page reaches `loams.live.v1`.
 *
 * Today Live listens on its own engine port (needs the TiKV stack), and the
 * desktop protocol handler routes the `/loams.live.v1.` prefix there, so the
 * console's own Connect transport already reaches it. LV1 (design 45) moves
 * Live to the main port with an embedded store: this function then returns the
 * same transport, or whatever the new route needs, and nothing else changes.
 */
export function liveTransport(deps: { transport: Transport }): Transport {
  return deps.transport;
}
