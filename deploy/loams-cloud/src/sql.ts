// The control DB port (design 78 §25.3, D1717). In the Worker it is
// @tidbcloud/serverless's `connect({ url, fullResult: true })`, called from the
// control Durable Object placed near the Starter instance (lc0-report §4: writes
// from a DO with a location hint near aws-us-east-1 take ~20 ms, from the edge
// ~280 ms). In tests it is the in-memory store or test/harness/tidb.ts.
//
// The write rule (LC0 Task 3, "aborted writes commit"): a timeout or a lost
// response never means "not written". So every write is one statement that is
// safe to repeat: an insert with a Worker-minted id or a natural key
// (INSERT IGNORE / ON DUPLICATE KEY UPDATE), or an update guarded by the state it
// moves from, with `rowsAffected` telling the caller whether it won.

export interface SqlResult {
  rows: Record<string, unknown>[];
  rowsAffected: number;
}

export interface Sql {
  execute(sql: string, params?: readonly unknown[]): Promise<SqlResult>;
}

/** MySQL's duplicate-key error, as the HTTP driver and the mysql client report it. */
export function isDuplicateKey(e: unknown): boolean {
  const msg = e instanceof Error ? e.message : String(e);
  return /\b1062\b|Duplicate entry/i.test(msg);
}
