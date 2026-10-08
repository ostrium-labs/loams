// Adapts the pure SQL tool definitions (pgTools, wesqlTools) to the agent's tool registry (Task 28).
import type { ToolDef as AgentToolDef } from "../agent/tools";
import { type PgToolCtx, pgTools } from "./pg";
import { type WesqlToolCtx, wesqlTools } from "./wesql";

export function sqlAgentTools(ctx: PgToolCtx & WesqlToolCtx): AgentToolDef[] {
	return [
		...pgTools.map(
			(t): AgentToolDef => ({
				name: t.name,
				description: t.description,
				risk: t.risk,
				schema: t.schema,
				run: (_c, args) => t.run(ctx, args),
			}),
		),
		...wesqlTools.map(
			(t): AgentToolDef => ({
				name: t.name,
				description: t.description,
				risk: t.risk,
				schema: t.schema,
				run: (_c, args) => t.run(ctx, args),
			}),
		),
	];
}
