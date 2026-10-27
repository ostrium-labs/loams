// The run's tool policy (design §72 §7.1–§7.2, D1430, D1431; plan FT1, FT0 Task 6).
//
// Until the Place record exists, a run's policy is the D1431 defaults tightened by the
// repository's `.loams/factory.toml`:
//
//   [tools]
//   deny = ["merge_change_request"]     # never offered to any agent
//   approve = ["open_change_request"]   # always asks the person
//
//   [agents.reviewer]
//   tools = ["get_thread_diff", "post_review_comment"]   # this agent's allow-list
//
// The file can only tighten: a destructive tool is `approve` whatever it says, and
// nothing in it turns the plan, merge and deploy approvals off.
import { parse } from 'smol-toml';

export type Risk = 'read' | 'write' | 'egress' | 'destructive';
export type Decision = 'allow' | 'approve' | 'deny';

export interface FactoryPolicy {
  deny: Set<string>;
  approve: Set<string>;
  /** Per agent: the tools it may use; absent means the agent's own default list. */
  agents: Map<string, Set<string>>;
}

export class PolicyError extends Error {
  override name = 'PolicyError';
}

const stringList = (value: unknown, where: string): string[] => {
  if (value === undefined) return [];
  if (!Array.isArray(value) || !value.every((v) => typeof v === 'string')) {
    throw new PolicyError(`${where} must be a list of tool names`);
  }
  return value as string[];
};

export const DEFAULT_POLICY: FactoryPolicy = {
  deny: new Set(),
  approve: new Set(),
  agents: new Map(),
};

/** Parse `.loams/factory.toml`. An empty file is the defaults. */
export function parsePolicy(text: string): FactoryPolicy {
  let doc: Record<string, unknown>;
  try {
    doc = parse(text) as Record<string, unknown>;
  } catch (error) {
    throw new PolicyError(`.loams/factory.toml: ${(error as Error).message}`);
  }
  const tools = (doc.tools ?? {}) as Record<string, unknown>;
  const agents = new Map<string, Set<string>>();
  for (const [agent, section] of Object.entries((doc.agents ?? {}) as Record<string, unknown>)) {
    const list = (section as Record<string, unknown> | undefined)?.tools;
    if (list !== undefined) agents.set(agent, new Set(stringList(list, `agents.${agent}.tools`)));
  }
  return {
    deny: new Set(stringList(tools.deny, 'tools.deny')),
    approve: new Set(stringList(tools.approve, 'tools.approve')),
    agents,
  };
}

/**
 * What the gate does with `tool` of `risk` for `agent` (D1431): read allow; write allow;
 * egress approve; destructive approve, always. The policy file denies, asks, or narrows
 * an agent's list; it never loosens.
 */
export function decide(policy: FactoryPolicy, agent: string, tool: string, risk: Risk): Decision {
  const listed = policy.agents.get(agent);
  if (listed && !listed.has(tool)) return 'deny';
  if (policy.deny.has(tool)) return 'deny';
  if (risk === 'destructive' || risk === 'egress') return 'approve';
  if (policy.approve.has(tool)) return 'approve';
  return 'allow';
}
