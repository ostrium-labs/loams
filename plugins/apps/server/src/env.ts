/**
 * Environment lookup with a deprecated fallback.
 *
 * The BI backend settings were once `SUPERSET_*`. They are now `LOAMS_BI_*`; the
 * old names are still read when the new one is unset, with a single warning per
 * variable so a stale deployment shows up in the logs without being flooded.
 */
const warned = new Set<string>();

export function readEnvWithFallback(name: string, deprecated: string): string | undefined {
  const current = process.env[name];
  if (current !== undefined && current !== "") return current;
  const legacy = process.env[deprecated];
  if (legacy === undefined || legacy === "") return undefined;
  if (!warned.has(deprecated)) {
    warned.add(deprecated);
    // stderr, not stdout: stdout is reserved for JSON-RPC framing under ENABLE_MCP.
    process.stderr.write(
      `[loams-plugins] ${deprecated} is deprecated and will be removed; set ${name} instead.\n`,
    );
  }
  return legacy;
}
