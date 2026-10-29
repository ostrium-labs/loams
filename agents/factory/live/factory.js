// The Live app `factory` (design §72 §6.5, D1436; plan FT1, FT0 Task 9): the run
// record, table `factory_runs`, one document per factory run. The workflow writes it
// with `runs:record` at every transition; the console, Studio and phones read it with
// `runs:list` and `runs:get` (and subscribe through Live).
//
// `runs:record` is idempotent: each transition carries the run's sequence number, and a
// transition at or below the stored one changes nothing, so a step the workflow replays
// after a crash writes the record once.
//
// One ES module for the Live runtime (design §45 §3.3); no imports beyond loams:server.
import { mutation, query, v } from 'loams:server';

const TABLE = 'factory_runs';

/** The stages of design §72 §3, then `close`. */
const STAGES = ['intent', 'plan', 'build', 'review', 'ci', 'merge', 'deploy', 'close'];
/** design §72 §6.5. */
const STATES = ['open', 'waiting', 'paused', 'succeeded', 'failed', 'killed'];

/** The fields a transition may set; anything else is refused. */
const FIELDS = [
  'project',
  'place',
  'intent',
  'stage',
  'state',
  'planUrl',
  'threads',
  'pr',
  'checks',
  'deploy',
  'approvals',
  'budget',
  'error',
];

function check(patch) {
  if (patch === null || typeof patch !== 'object' || Array.isArray(patch)) {
    throw new Error('runs:record: patch is an object');
  }
  for (const key of Object.keys(patch)) {
    if (!FIELDS.includes(key)) throw new Error(`runs:record: unknown field ${key}`);
    if (patch[key] === undefined) throw new Error(`runs:record: ${key} is undefined`);
  }
  if ('stage' in patch && !STAGES.includes(patch.stage)) {
    throw new Error(`runs:record: unknown stage ${patch.stage}`);
  }
  if ('state' in patch && !STATES.includes(patch.state)) {
    throw new Error(`runs:record: unknown state ${patch.state}`);
  }
}

async function byRun(ctx, run) {
  return await ctx.db
    .query(TABLE)
    .withIndex('by_run', (q) => q.eq('run', run))
    .first();
}

export const runs = {
  // Apply one transition: `seq` must be above the stored one, else nothing changes.
  record: mutation({
    args: { run: v.string(), seq: v.int64(), patch: v.any() },
    handler: async (ctx, { run, seq, patch }) => {
      check(patch);
      const now = Date.now();
      const existing = await byRun(ctx, run);
      if (!existing) {
        await ctx.db.insert(TABLE, {
          state: 'open',
          stage: 'intent',
          ...patch,
          run,
          seq,
          createdAt: now,
          updatedAt: now,
        });
        return { applied: true, seq };
      }
      if (existing.seq >= seq) return { applied: false, seq: existing.seq };
      await ctx.db.patch(existing._id, { ...patch, seq, updatedAt: now });
      return { applied: true, seq };
    },
  }),
  // The newest runs first.
  list: query({
    args: { limit: v.optional(v.int64()) },
    handler: async (ctx, { limit }) => {
      const n = limit === undefined ? 50 : Number(limit);
      return await ctx.db
        .query(TABLE)
        .withIndex('by_updated')
        .order('desc')
        .take(Math.max(1, Math.min(n, 200)));
    },
  }),
  get: query({
    args: { run: v.string() },
    handler: async (ctx, { run }) => await byRun(ctx, run),
  }),
};
