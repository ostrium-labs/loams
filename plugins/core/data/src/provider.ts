/**
 * Where a widget's rows come from.
 *
 * The dashboard was built against exactly one source -- the BI backend, through the
 * control plane -- and `_executeQuery` reached for it unconditionally. That is
 * the wrong shape now: Loams has its own live state, exposed as server-streaming
 * `Watch*` RPCs, and a dashboard tile reading operations or approvals wants
 * *that*, not an analytics copy of it.
 *
 * So a source is a registered provider keyed on `widget.data.source`, and
 * The BI backend becomes the default rather than the only option. Nothing about the
 * renderer changes: `ctx.render` and `ctx.flow` still receive rows.
 */

/** The row shape the renderers consume, matching the BI backend's chart-data reply. */
export interface QueryResult {
  data: Record<string, unknown>[];
  rowcount: number;
}

export interface DataProvider {
  /** Matches `widget.data.source`. */
  readonly source: string;

  /**
   * Whether this source is a live projection rather than a stored table.
   *
   * A live provider is never served from the five-minute result cache: the
   * whole point is that the next read reflects what changed since the last
   * one, and a cached row is exactly the staleness the caller is paying to
   * avoid. It also opts out of in-flight de-duplication, because two widgets on
   * the same stream should each observe the current state rather than share a
   * promise that may have been created before it changed.
   */
  readonly live?: boolean;

  query(widget: any, params: Record<string, unknown>): Promise<QueryResult>;
}

/**
 * A provider that never claims a widget.
 *
 * Used as the fallback when `widget.data` names no registered source, so a
 * misspelled `source` reaches the BI backend -- where it will fail loudly against a
 * real dataset -- instead of silently rendering an empty tile.
 */
export const EMPTY_RESULT: QueryResult = { data: [], rowcount: 0 };