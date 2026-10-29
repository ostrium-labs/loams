// Applies the control-DB migrations in schema/ (LC2a Task 1). The Worker imports
// each .sql file as text (wrangler's Text rule); tests read them from disk.

import type { Sql } from "./sql.ts";

export interface Migration {
  version: number;
  name: string;
  text: string;
}

/** Splits a migration into statements: drops `--` comments, then splits on `;`. */
export function splitStatements(text: string): string[] {
  const noComments = text
    .split("\n")
    .map((line) => {
      const i = line.indexOf("--");
      return i === -1 ? line : line.slice(0, i);
    })
    .join("\n");
  return noComments
    .split(";")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** Applies every migration not yet recorded. Each statement is idempotent, so a retry is safe. */
export async function migrate(sql: Sql, migrations: readonly Migration[]): Promise<number[]> {
  let applied = new Set<number>();
  try {
    const r = await sql.execute("SELECT version FROM schema_migrations");
    applied = new Set(r.rows.map((row) => Number(row.version)));
  } catch {
    // No schema_migrations yet: a fresh database.
  }
  const done: number[] = [];
  for (const m of [...migrations].sort((a, b) => a.version - b.version)) {
    if (applied.has(m.version)) continue;
    for (const stmt of splitStatements(m.text)) await sql.execute(stmt);
    done.push(m.version);
  }
  return done;
}
