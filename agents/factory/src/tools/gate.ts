// The tool gate (design §72 §7.2, D1431; plan FT1, FT0 Task 6): every tool a factory
// agent gets passes through `gate`, which reads the run's policy and the tool's risk.
// `approve` sets Mastra's `requireApproval`, which the ACP bridge turns into an agentd
// approval in Studio's queue; `deny` keeps the tool from the agent altogether.
//
// This is the client half. agentd still refuses a destructive call without a settled
// approval whose hash matches (`MergeChangeRequest`), whatever this process does.
import type { ToolAction } from '@mastra/core/tools';

import { type Decision, decide, type FactoryPolicy, type Risk } from '../policy.js';

// biome-ignore lint/suspicious/noExplicitAny: Mastra's tool type is generic over its schemas.
type AnyTool = ToolAction<any, any, any, any, any, any>;

/** agentd's tool risks (`loams-agentd-mcp` `risk_of`); unknown tools ask. */
const AGENTD_RISK: Record<string, Risk> = {
  whoami: 'read',
  list_projects: 'read',
  list_harnesses: 'read',
  list_models: 'read',
  list_chats: 'read',
  get_chat: 'read',
  read_chat: 'read',
  wait_for_turn: 'read',
  get_thread_diff: 'read',
  get_approval: 'read',
  get_checks: 'read',
  get_check_log: 'read',
  create_chat: 'write',
  create_chats: 'write',
  send_message: 'write',
  send_messages: 'write',
  interrupt_chat: 'write',
  respond_to_input: 'write',
  archive_chat: 'write',
  post_review_comment: 'write',
  open_change_request: 'write',
  request_approval: 'write',
  close_change_request: 'write',
  merge_change_request: 'destructive',
  deploy_live_app: 'destructive',
};

/** A tool's risk: the agentd table, else destructive (so it always asks). */
export function riskOf(name: string): Risk {
  return AGENTD_RISK[name] ?? 'destructive';
}

/** The bare tool name of an MCP-namespaced one (`loams-agentd_get_checks`). */
export function bareName(name: string): string {
  const at = name.indexOf('_');
  return name.startsWith('loams-agentd_') && at > 0 ? name.slice(at + 1) : name;
}

export interface GateOptions {
  agent: string;
  policy: FactoryPolicy;
  /** The risk, when the tool is not one of agentd's. */
  risk?: Risk;
}

/** Gate one tool: the tool (maybe with `requireApproval`), or `undefined` when denied. */
export function gate<T extends AnyTool>(
  name: string,
  tool: T,
  options: GateOptions,
): T | undefined {
  const risk = options.risk ?? riskOf(bareName(name));
  const decision: Decision = decide(options.policy, options.agent, bareName(name), risk);
  if (decision === 'deny') return undefined;
  if (decision === 'approve') {
    // A copy that keeps the tool's prototype (Mastra's `Tool` class), so nothing the
    // caller holds is changed.
    const copy = Object.assign(Object.create(Object.getPrototypeOf(tool)), tool) as T;
    (copy as { requireApproval?: unknown }).requireApproval = true;
    return copy;
  }
  return tool;
}

/** Gate a tool set for one agent, dropping the denied ones. */
export function gateTools(
  tools: Record<string, AnyTool>,
  options: Omit<GateOptions, 'risk'>,
): Record<string, AnyTool> {
  const out: Record<string, AnyTool> = {};
  for (const [name, tool] of Object.entries(tools)) {
    const gated = gate(name, tool, options);
    if (gated) out[name] = gated;
  }
  return out;
}
