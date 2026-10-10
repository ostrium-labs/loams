// Maps the Loams Live tools (agent-tools/live.ts, Task 24) onto the agent's
// ToolDef registry (D675). Risk tags come from the tool's `access`.
import { type LiveTool, liveTools } from "../agent-tools/live";
import type { ToolDef } from "./tools";

export interface LiveToolDeps {
	/** A request to the console origin; the protocol handler routes Live calls to the engine. */
	fetch: typeof fetch;
	/** `loams-app://console` in the app: the same routing as the Live page. */
	liveUrl: string;
}

export function liveToolDefs(
	deps: LiveToolDeps,
	tools: readonly LiveTool[] = liveTools,
): ToolDef[] {
	return tools.map((t) => ({
		name: t.name,
		description: t.description,
		risk: t.access,
		schema: t.inputSchema,
		run: (ctx, args) =>
			t.run(args, {
				liveUrl: deps.liveUrl,
				fetch: ((url: string | URL | Request, init?: RequestInit) =>
					deps.fetch(url, {
						...init,
						signal: init?.signal
							? AbortSignal.any([init.signal, ctx.signal])
							: ctx.signal,
					})) as typeof fetch,
			}),
	}));
}
