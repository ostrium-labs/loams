import { inspect } from "node:util";
import { Logger, type Exporter, type Formatter, type Message } from "cordis";

/**
 * A minimal cordis core log `Exporter` that writes to **stderr** instead of stdout.
 *
 * Why this exists: `apps/server/src/index.ts` runs the HTTP API and the MCP stdio
 * server in the same process. MCP's stdio transport uses stdout for framed
 * JSON-RPC, so anything else written there corrupts the transport. The
 * `@cordisjs/plugin-logger-console` Node entry writes to stdout,
 * so it is only safe when MCP is disabled. When MCP is enabled we register this
 * exporter instead and leave stdout untouched.
 *
 * Formatting is deliberately delegated to cordis core (`Logger.format`) rather than
 * reimplemented here, so `%s` / `%d` / `%o` placeholder handling stays consistent
 * with the rest of the ecosystem.
 */
export class StderrExporter implements Exporter {
  /** Follow stderr's colour capability rather than stdout's, since that is where we write. */
  colors: number | false = process.stderr.isTTY ? 1 : false;

  /** Core's default `o`/`O` formatter is JSON.stringify, which renders Errors as `{}`; inspect is more useful. */
  formatters: Record<string, Formatter> = {
    o: (value) => inspect(value, { depth: Infinity, compact: true, breakLength: Infinity }),
    O: (value) => inspect(value, { depth: Infinity, compact: true, breakLength: Infinity }),
  };

  export(message: Message): void {
    process.stderr.write(Logger.format(this, message) + "\n");
  }
}
