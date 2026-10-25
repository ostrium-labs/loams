// agentd's tools, through its MCP shim (design §72 §5.1 item 2, D1424; plan FT1, FT0
// Task 6). The shim (`loams-agentd mcp`) holds no credential either: it forwards each
// call to the daemon with the run's scoped token, and the daemon runs the tool where the
// keyring is.
//
// agentd hands the shim's definition to an ACP harness in `session/new`'s `mcpServers`
// (command, args and the three `LOAMS_AGENTD_*` variables); `agentdServerFromEnv` is the
// fallback for a host started by hand next to a daemon.
import { MCPClient } from '@mastra/mcp';

export const AGENTD_SERVER = 'loams-agentd';

/** The variables the shim needs, and the only ones passed to it. */
export const SHIM_ENV = ['LOAMS_AGENTD_PORT', 'LOAMS_AGENTD_RUN_TOKEN', 'LOAMS_AGENTD_SESSION_ID'];

export interface ShimDefinition {
  command: string;
  args: string[];
  env: Record<string, string>;
}

/** An ACP `mcpServers` entry (stdio) as agentd sends it. */
export interface AcpStdioServer {
  name: string;
  command: string;
  args?: string[];
  env?: { name: string; value: string }[];
}

/** The shim among an ACP session's MCP servers. */
export function shimFromAcp(servers: AcpStdioServer[]): ShimDefinition | undefined {
  const shim = servers.find((s) => s.name === AGENTD_SERVER || s.name === 'loams-desktop');
  if (!shim) return undefined;
  const env: Record<string, string> = {};
  for (const { name, value } of shim.env ?? []) if (SHIM_ENV.includes(name)) env[name] = value;
  return { command: shim.command, args: shim.args ?? [], env };
}

/** `loams-agentd mcp` with the shim's variables from this process's environment. */
export function agentdServerFromEnv(
  env: NodeJS.ProcessEnv = process.env,
  command = env.LOAMS_AGENTD_EXECUTABLE ?? 'loams-agentd',
): ShimDefinition {
  const out: Record<string, string> = {};
  for (const name of SHIM_ENV) {
    const value = env[name];
    if (value) out[name] = value;
  }
  return { command, args: ['mcp'], env: out };
}

/**
 * An MCP client for the shim. The subprocess gets only the shim's variables (no
 * inherited environment beyond `PATH`), so nothing else of the host's leaks to it.
 */
export function createAgentdMcp(shim: ShimDefinition): MCPClient {
  return new MCPClient({
    id: 'loams-factory-agentd',
    servers: {
      [AGENTD_SERVER]: {
        command: shim.command,
        args: shim.args,
        env: { ...shim.env, ...(process.env.PATH ? { PATH: process.env.PATH } : {}) },
        inheritDefaultEnv: false,
      },
    },
  });
}
