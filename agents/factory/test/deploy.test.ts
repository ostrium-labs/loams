// FT0 Task 8 (plan FT1): the deploy step's client half. agentd's own tests
// (`crates/loams-agentd-sessions/tests/factory_deploy.rs`) cover the deploy, the health
// check, the rollback and the approval check where the credential lives; these check
// that the host asks, waits for a person, and never calls the deploy tool before.
import { describe, expect, it } from 'vitest';

import {
  type AgentdCall,
  DEPLOY_TOOL,
  DeployError,
  deployPayload,
  liveDeployStep,
  PolicyError,
  parsePolicy,
  riskOf,
} from '../src/index.js';

const COMMIT = 'c0ffee'.padEnd(40, '0');

/** A fake agentd: approvals by id, and a deploy that records each call. */
function fakeAgentd(state: 'pending' | 'approved' | 'denied' | 'used' = 'pending') {
  const calls: { tool: string; args: Record<string, unknown> }[] = [];
  let approval = state;
  let deployed = 0;
  const call: AgentdCall = async (tool, args) => {
    calls.push({ tool, args });
    switch (tool) {
      case 'request_approval':
        return { id: 'fa-1', state: 'pending' };
      case 'get_approval':
        return { id: args.approval_id, state: approval };
      case DEPLOY_TOOL: {
        if (approval !== 'approved' && approval !== 'used') {
          throw new Error('approval_required');
        }
        const resumed = approval === 'used';
        if (!resumed) deployed += 1;
        approval = 'used';
        return {
          target: 'live',
          app: args.app,
          commit: args.commit,
          deploymentId: 'd1',
          bundleSha256: 'ab'.repeat(32),
          previousDeploymentId: 'd0',
          state: 'active',
          health: 'active',
          resumed,
        };
      }
      default:
        throw new Error(`unexpected tool ${tool}`);
    }
  };
  return {
    call,
    calls,
    settle: (s: typeof approval) => {
      approval = s;
    },
    deployed: () => deployed,
  };
}

describe('the Live deploy step', () => {
  it('asks for a deploy approval bound to the target, app and commit', async () => {
    const agentd = fakeAgentd();
    const step = await liveDeployStep(agentd.call, { app: 'notes', commit: COMMIT, message: '#7' });
    expect(step).toEqual({ status: 'waiting', approvalId: 'fa-1' });
    expect(agentd.calls).toHaveLength(1);
    expect(agentd.calls[0]?.tool).toBe('request_approval');
    expect(agentd.calls[0]?.args.kind).toBe('deploy');
    expect(agentd.calls[0]?.args.payload).toEqual(deployPayload('notes', COMMIT));
    expect(agentd.calls[0]?.args.payload).toEqual({ target: 'live', app: 'notes', commit: COMMIT });
  });

  it('deploy_requires_settled_approval', async () => {
    const agentd = fakeAgentd('pending');
    const input = { app: 'notes', commit: COMMIT, approvalId: 'fa-1' };
    expect(await liveDeployStep(agentd.call, input)).toEqual({
      status: 'waiting',
      approvalId: 'fa-1',
    });
    agentd.settle('denied');
    expect(await liveDeployStep(agentd.call, input)).toEqual({
      status: 'denied',
      approvalId: 'fa-1',
    });
    expect(agentd.calls.some((c) => c.tool === DEPLOY_TOOL)).toBe(false);
    agentd.settle('approved');
    const done = await liveDeployStep(agentd.call, input);
    expect(done.status).toBe('done');
    if (done.status !== 'done') return;
    expect(done.record).toMatchObject({
      target: 'live',
      app: 'notes',
      environment: 'live',
      commit: COMMIT,
      deploymentId: 'd1',
      previousDeploymentId: 'd0',
      artifactDigest: 'ab'.repeat(32),
      state: 'active',
      approvalId: 'fa-1',
      resumed: false,
    });
    const deploy = agentd.calls.find((c) => c.tool === DEPLOY_TOOL);
    expect(deploy?.args).toEqual({ approval_id: 'fa-1', app: 'notes', commit: COMMIT });
  });

  it('deploy_step_is_idempotent', async () => {
    const agentd = fakeAgentd('approved');
    const input = { app: 'notes', commit: COMMIT, approvalId: 'fa-1' };
    const first = await liveDeployStep(agentd.call, input);
    const second = await liveDeployStep(agentd.call, input);
    expect(agentd.deployed()).toBe(1);
    expect(first.status === 'done' && first.record.resumed).toBe(false);
    expect(second.status === 'done' && second.record.resumed).toBe(true);
    expect(second.status === 'done' && second.record.deploymentId).toBe('d1');
  });

  it('refuses a short commit, a bad app and a policy that denies the deploy', async () => {
    const agentd = fakeAgentd('approved');
    await expect(liveDeployStep(agentd.call, { app: 'notes', commit: 'c0ffee' })).rejects.toThrow(
      DeployError,
    );
    await expect(liveDeployStep(agentd.call, { app: 'Notes App', commit: COMMIT })).rejects.toThrow(
      DeployError,
    );
    const policy = parsePolicy('[tools]\ndeny = ["deploy_live_app"]\n');
    await expect(
      liveDeployStep(agentd.call, { app: 'notes', commit: COMMIT, approvalId: 'fa-1' }, policy),
    ).rejects.toThrow(PolicyError);
    expect(agentd.calls).toHaveLength(0);
  });

  it('the deploy tool is destructive in the gate', () => {
    expect(riskOf(DEPLOY_TOOL)).toBe('destructive');
  });
});
