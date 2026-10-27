// A test double for the control DB's HTTP driver: real TiDB (tidb-server on
// unistore, scripts/test-tidb.sh) reached through the mysql client binary, so the
// suite needs no npm dependency. Tests that need it skip unless
// LOAMS_CLOUD_TEST_TIDB is set, as `host:port` (user root, no password).
//
// Parameters are bound client-side with `?` placeholders, as
// @tidbcloud/serverless does, and every call is its own session, as every HTTP
// driver request is.

import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Sql, SqlResult } from "../../src/sql.ts";
import type { Migration } from "../../src/schema.ts";

export const TIDB = process.env.LOAMS_CLOUD_TEST_TIDB ?? "";
export const skipNoTidb = TIDB ? false : "set LOAMS_CLOUD_TEST_TIDB (scripts/test-tidb.sh)";

const here = dirname(fileURLToPath(import.meta.url));
export const schemaDir = join(here, "..", "..", "schema");

export function loadMigrations(): Migration[] {
  return readdirSync(schemaDir)
    .filter((f) => /^\d{4}_.+\.sql$/.test(f))
    .sort()
    .map((f) => ({ version: Number(f.slice(0, 4)), name: f.replace(/\.sql$/, ""), text: readFileSync(join(schemaDir, f), "utf8") }));
}

function client(): string {
  for (const bin of ["mysql", "mariadb"]) {
    try {
      execFileSync(bin, ["--version"], { stdio: "ignore" });
      return bin;
    } catch {}
  }
  throw new Error("no mysql or mariadb client on PATH");
}
const BIN = TIDB ? client() : "mysql";

function literal(v: unknown): string {
  if (v === null || v === undefined) return "NULL";
  if (typeof v === "number" || typeof v === "bigint") return String(v);
  if (typeof v === "boolean") return v ? "1" : "0";
  const s = typeof v === "string" ? v : JSON.stringify(v);
  return "'" + s.replace(/\\/g, "\\\\").replace(/'/g, "\\'").replace(/\n/g, "\\n").replace(/\r/g, "\\r").replace(/\0/g, "\\0") + "'";
}

export function bind(sql: string, params: readonly unknown[] = []): string {
  let i = 0;
  const out = sql.replace(/\?/g, () => {
    if (i >= params.length) throw new Error(`too few parameters for: ${sql}`);
    return literal(params[i++]);
  });
  if (i !== params.length) throw new Error(`too many parameters for: ${sql}`);
  return out;
}

function unxml(s: string): string {
  return s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&#(\d+);/g, (_, n) => String.fromCharCode(Number(n))).replace(/&amp;/g, "&");
}

function parseXml(xml: string): Record<string, unknown>[][] {
  const sets: Record<string, unknown>[][] = [];
  for (const rs of xml.matchAll(/<resultset[^>]*>([\s\S]*?)<\/resultset>|<resultset[^>]*\/>/g)) {
    const rows: Record<string, unknown>[] = [];
    for (const row of (rs[1] ?? "").matchAll(/<row>([\s\S]*?)<\/row>/g)) {
      const rec: Record<string, unknown> = {};
      for (const f of row[1].matchAll(/<field name="([^"]*)"(\s+xsi:nil="true"\s*\/>|\s*\/>|>([\s\S]*?)<\/field>)/g)) {
        rec[unxml(f[1])] = f[2].includes("xsi:nil") ? null : unxml(f[3] ?? "");
      }
      rows.push(rec);
    }
    sets.push(rows);
  }
  return sets;
}

export class TidbCli implements Sql {
  readonly database: string;
  readonly statements: string[] = [];
  constructor(database: string) {
    this.database = database;
  }

  async execute(sql: string, params: readonly unknown[] = []): Promise<SqlResult> {
    const text = bind(sql, params);
    this.statements.push(text);
    const [host, port] = TIDB.split(":");
    const args = ["-h", host, "-P", port ?? "4000", "-uroot", "--xml", "-D", this.database, "-e", `${text};\nSELECT ROW_COUNT() AS __rc`];
    let out: string;
    try {
      out = execFileSync(BIN, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    } catch (e) {
      const err = e as { stderr?: string };
      throw new Error(String(err.stderr ?? e).replace(/^.*Deprecated program name.*\n/m, "").trim());
    }
    const sets = parseXml(out);
    const rc = Number((sets.at(-1)?.[0] ?? {}).__rc ?? 0);
    const rows = sets.length > 1 ? sets[0] : [];
    return { rows, rowsAffected: rc < 0 ? 0 : rc };
  }
}

function admin(stmt: string): void {
  const [host, port] = TIDB.split(":");
  execFileSync(BIN, ["-h", host, "-P", port ?? "4000", "-uroot", "-e", stmt], { stdio: ["ignore", "ignore", "pipe"] });
}

/** A fresh, empty database for one test file; drop it with `dispose`. */
export function freshDatabase(): { sql: TidbCli; dispose: () => void } {
  const name = `lc_test_${randomBytes(6).toString("hex")}`;
  admin(`CREATE DATABASE ${name}`);
  return { sql: new TidbCli(name), dispose: () => admin(`DROP DATABASE IF EXISTS ${name}`) };
}
