// The deploy step on Loams Live (design §72 §8, D1434; plan FT1, FT0 Task 8).
//
// The deploy itself runs in agentd (`deploy_live_app`), which holds the Live credential
// (owner ruling, 2026-10-24): this process never sees it. The step here only asks the
// person for the deploy approval, bound to `{target: "live", app, commit}`, waits for it
// to be settled, and then calls agentd with the approval's id. agentd reads the bundle
// and schema from that commit, deploys, checks health and rolls back on failure, and
// refuses the call without a settled, approved approval for exactly that payload.
//
// The step is safe to run again: with no approval it asks once (the workflow keeps the
// returned id), and with a spent approval agentd answers the deployment the earlier call
// made instead of deploying twice.
import { type FactoryPolicy, PolicyError } from '../policy.js';

export const DEPLOY_TOOL = 'deploy_live_app';

/** Calls one of agentd's tools (through the MCP shim) and answers its JSON result. */
export type AgentdCall = (tool: string, args: Record<string, unknown>) => Promise<unknown>;

/** agentd's `LiveDeployState`. */
export type LiveDeployState =
  | 'active'
  | 'rolled_back'
  | 'unhealthy'
  | 'failed'
  | 'pending'
  | 'superseded';

/**
 * The deploy record kept on the run (D1167's fields) until `loams.deploy.v1` exists:
 * the target and app, the environment, the artifact digest (the bundle's SHA-256), the
 * commit, the Live deployment and the one it replaced, the state and the approval.
 */
export interface DeployRecord {
  target: 'live';
  app: string;
  environment: string;
  commit: string;
  deploymentId: string;
  previousDeploymentId: string;
  artifactDigest: string;
  state: LiveDeployState;
  health: string;
  failureReason?: string;
  approvalId: string;
  /** An earlier call had deployed it; this is that deployment, found again. */
  resumed: boolean;
}

export type DeployStepResult =
  | { status: 'waiting'; approvalId: string }
  | { status: 'denied'; approvalId: string }
  | { status: 'done'; approvalId: string; record: DeployRecord };

export interface LiveDeployInput {
  app: string;
  /** The full commit id of the merge. */
  commit: string;
  /** For the deployment's record, such as the pull request's title. */
  message?: string;
  /** The approval asked by an earlier run of this step. */
  approvalId?: string;
  /** The chat that asks; agentd defaults to the host's own thread. */
  chat?: string;
}

export class DeployError extends Error {
  override name = 'DeployError';
}

/** What a deploy approval must carry: agentd hashes exactly this. */
export function deployPayload(app: string, commit: string) {
  return { target: 'live', app, commit } as const;
}

const FULL_COMMIT = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const APP_NAME = /^[a-z0-9_-]{1,63}$/;

export function deploySummary(input: LiveDeployInput): string {
  const note = input.message?.trim() ? ` (${input.message.trim()})` : '';
  return `Deploy commit ${input.commit.slice(0, 12)} to the Live app "${input.app}"${note}. agentd checks its health and rolls back to the previous deployment if the check fails.`;
}

const record = (value: unknown): Record<string, unknown> => {
  if (value && typeof value === 'object') return value as Record<string, unknown>;
  throw new DeployError(`agentd answered ${JSON.stringify(value)}`);
};
const text = (value: unknown) => (typeof value === 'string' ? value : '');

/**
 * Run the deploy step once. Without `approvalId` it asks for the approval and answers
 * `waiting`; with one it answers `waiting` or `denied` until a person approves, and then
 * deploys through agentd.
 */
export async function liveDeployStep(
  call: AgentdCall,
  input: LiveDeployInput,
  policy?: FactoryPolicy,
): Promise<DeployStepResult> {
  const app = input.app.trim();
  const commit = input.commit.trim().toLowerCase();
  if (!APP_NAME.test(app)) throw new DeployError(`not a Live app name: ${JSON.stringify(app)}`);
  if (!FULL_COMMIT.test(commit)) throw new DeployError('a deploy names the full commit id');
  if (policy?.deny.has(DEPLOY_TOOL)) {
    throw new PolicyError(`.loams/factory.toml denies ${DEPLOY_TOOL}: this run does not deploy`);
  }
  const chat = input.chat ? { chat: input.chat } : {};

  if (!input.approvalId) {
    const asked = record(
      await call('request_approval', {
        ...chat,
        kind: 'deploy',
        summary: deploySummary({ ...input, app, commit }),
        payload: deployPayload(app, commit),
      }),
    );
    return { status: 'waiting', approvalId: text(asked.id) };
  }

  const approvalId = input.approvalId;
  const approval = record(await call('get_approval', { ...chat, approval_id: approvalId }));
  switch (approval.state) {
    case 'pending':
      return { status: 'waiting', approvalId };
    case 'denied':
      return { status: 'denied', approvalId };
    case 'approved':
    case 'used':
      break;
    default:
      throw new DeployError(`approval ${approvalId} is ${JSON.stringify(approval.state)}`);
  }
  const reply = record(
    await call(DEPLOY_TOOL, {
      ...chat,
      approval_id: approvalId,
      app,
      commit,
      ...(input.message ? { message: input.message } : {}),
    }),
  );
  return {
    status: 'done',
    approvalId,
    record: {
      target: 'live',
      app: text(reply.app) || app,
      environment: 'live',
      commit: text(reply.commit) || commit,
      deploymentId: text(reply.deploymentId),
      previousDeploymentId: text(reply.previousDeploymentId),
      artifactDigest: text(reply.bundleSha256),
      state: text(reply.state) as LiveDeployState,
      health: text(reply.health),
      ...(text(reply.failureReason) ? { failureReason: text(reply.failureReason) } : {}),
      approvalId,
      resumed: reply.resumed === true,
    },
  };
}
