// The code-host agents, `forgejo` and `github` (design §72 §4.2): open the run's pull
// request, read its checks and logs, and merge it only behind the person's approval.
// Their tools come from agentd through the gate; agentd holds the code-host credentials.
import { Agent } from '@mastra/core/agent';
import type { MastraModelConfig } from '@mastra/core/llm';
import type { ToolAction } from '@mastra/core/tools';

import type { FactoryPolicy } from '../policy.js';
import { bareName, gateTools } from '../tools/gate.js';
import { UNTRUSTED_RULE } from '../untrusted.js';

// oxlint-disable-next-line typescript/no-explicit-any -- Mastra's tool type is generic over its schemas.
type AnyTool = ToolAction<any, any, any, any, any, any>;

export type CodeHost = 'forgejo' | 'github';

/** The agentd tools a code-host agent may have, before the policy narrows them. */
export const CODE_HOST_TOOLS = [
  'open_change_request',
  'get_checks',
  'get_check_log',
  'request_approval',
  'get_approval',
  'merge_change_request',
];

const LABEL: Record<CodeHost, string> = { forgejo: 'Forgejo', github: 'GitHub' };

export function codeHostInstructions(host: CodeHost): string {
  return [
    `You are the ${LABEL[host]} agent of the Loams Software Factory.`,
    `Open one pull request per run with open_change_request (provider ${host}), read its`,
    'checks with get_checks and a failing check log with get_check_log.',
    'Merge only with merge_change_request and the id of a merge approval the person has',
    'approved for exactly that pull request and head commit; never push to a protected',
    'branch, never merge without that approval.',
    UNTRUSTED_RULE,
  ].join(' ');
}

export function createCodeHostAgent(
  host: CodeHost,
  model: MastraModelConfig,
  agentdTools: Record<string, AnyTool>,
  policy: FactoryPolicy,
): Agent {
  const offered = Object.fromEntries(
    Object.entries(agentdTools).filter(([name]) => CODE_HOST_TOOLS.includes(bareName(name))),
  );
  return new Agent({
    id: host,
    name: LABEL[host],
    description: `Opens the run's ${LABEL[host]} pull request, reads its checks, and merges behind an approval.`,
    instructions: codeHostInstructions(host),
    model,
    tools: gateTools(offered, { agent: host, policy }),
  });
}
