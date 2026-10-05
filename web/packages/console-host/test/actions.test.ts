// The plugin action registry (AP1d Task 2; design §42 §5, D569, D636).
//
// The registry is the single description of each action: its name, its
// description, its input schema, the permission it needs and its execute
// closure. The console's WebMCP tools are generated from it, and the stdio MCP
// server (§30, D289) is meant to read the same rows — which is why the name
// rule here is the intersection both consumers require rather than either
// one's private rule.
//
// Enforcement is not re-implemented: `decideCall` and `decidePermission` in
// `permissions.ts` are the same primitives the sandbox bridge and the buttons
// go through, and an action that names a Connect method *derives* its
// permission from `METHOD_PERMISSIONS` instead of restating it.

import { describe, expect, it } from 'vitest';
import {
  ActionRegistry,
  type ActionSpec,
  type Admission,
  ManifestError,
  validateManifest,
} from '../src/index.js';

const ADMISSIONS: Record<string, Admission> = {
  jobs: {
    actions: ['jobs.list', 'jobs.cancel'],
    policy: { services: ['rpc.jobs'], permissions: ['jobs:read', 'operations:cancel'] },
  },
  ops: {
    actions: ['ops.drop'],
    policy: { services: ['rpc.operations'], permissions: ['operations:cancel'] },
  },
};

function spec(over: Partial<ActionSpec> = {}): ActionSpec {
  return {
    name: 'jobs.list',
    plugin: 'jobs',
    description: 'List the job queues.',
    risk: 'read',
    permission: 'jobs:read',
    execute: async () => ['default', 'retry'],
    ...over,
  };
}

function registryWith(...specs: ActionSpec[]): ActionRegistry {
  const registry = new ActionRegistry(ADMISSIONS);
  for (const s of specs) registry.register(s);
  return registry;
}

const call = (extra: Partial<Parameters<ActionRegistry['invoke']>[2]> = {}) => ({
  signal: new AbortController().signal,
  ...extra,
});

describe('the action registry', () => {
  it('registers_an_action_and_returns_its_disposer', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    const dispose = registry.register(spec());
    expect(registry.actions().map((a) => a.name)).toEqual(['jobs.list']);

    dispose();
    expect(registry.actions()).toEqual([]);
    // Idempotent, like `SlotRegistry.register`.
    expect(() => dispose()).not.toThrow();
  });

  it('is_non_transactional_so_a_refused_registration_keeps_the_earlier_ones', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    registry.register(spec());
    expect(() => registry.register(spec({ name: 'jobs.cancel', description: '' }))).toThrow();
    expect(registry.actions().map((a) => a.name)).toEqual(['jobs.list']);
  });

  it('refuses_a_duplicate_name', () => {
    const registry = registryWith(spec());
    expect(() => registry.register(spec({ description: 'A second one.' }))).toThrow(
      /"jobs.list" is already registered/,
    );
    expect(registry.actions()).toHaveLength(1);
  });

  it('refuses_an_unadmitted_plugin_before_it_can_probe_the_registry', () => {
    // The duplicate check answers "is this name taken", which is a question an
    // unadmitted plugin has no business asking. Admission comes first, so an
    // unadmitted plugin learns nothing about who else registered what.
    const registry = registryWith(spec());
    expect(() =>
      registry.register(spec({ plugin: 'sneaky', description: 'A second one.' })),
    ).toThrow(/sneaky is not admitted/);
    expect(registry.actions()).toHaveLength(1);
  });

  it('refuses_an_empty_malformed_or_overlong_name', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    for (const name of ['', 'jobs list', 'jobs/list', 'jobs:list', 'x'.repeat(129)]) {
      expect(() => registry.register(spec({ name })), name).toThrow(/name/);
    }
    expect(registry.actions()).toEqual([]);
  });

  it('refuses_an_empty_description', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    expect(() => registry.register(spec({ description: '   ' }))).toThrow(/description/);
  });

  it('refuses_an_action_that_declares_no_risk', () => {
    // The risk is what drives the approval gate, so defaulting an absent one to
    // `read` is a fail-*open* default on the one field the gate reads: an action
    // whose author forgot it would run un-gated. §39 §8 rule 1 has the agent
    // declare the risk; this refuses the declaration being missing.
    const registry = new ActionRegistry(ADMISSIONS);
    expect(() => registry.register(spec({ risk: undefined }))).toThrow(/risk/);
    expect(registry.actions()).toEqual([]);
  });

  it('refuses_an_unknown_permission', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    expect(() =>
      registry.register(spec({ permission: 'jobs:teleport' as unknown as 'jobs:read' })),
    ).toThrow(/unknown permission/);
  });

  it('refuses_an_action_the_plugin_did_not_declare', () => {
    // `slots` already fails this way (§37 §5.3); the manifest's `actions` list
    // is the same gate for the callable surface.
    const registry = new ActionRegistry(ADMISSIONS);
    expect(() => registry.register(spec({ name: 'jobs.purge' }))).toThrow(
      /jobs declares no action "jobs.purge"/,
    );
  });

  it('refuses_a_plugin_that_is_not_admitted_at_all', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    expect(() => registry.register(spec({ plugin: 'sneaky' }))).toThrow(/sneaky is not admitted/);
  });

  it('refuses_an_unadmitted_plugin_id_that_names_an_object_prototype_member', () => {
    // The admissions are a plain object, so `admissions['toString']` would
    // otherwise answer `Object.prototype.toString` — truthy — and the refusal
    // would come back as a `TypeError` about `undefined.includes` instead of the
    // admission message. A refusal is still a refusal, but an unreadable one.
    const registry = new ActionRegistry(ADMISSIONS);
    for (const plugin of ['toString', 'constructor', '__proto__', 'hasOwnProperty']) {
      expect(() => registry.register(spec({ plugin })), plugin).toThrow(
        new RegExp(`${plugin} is not admitted`),
      );
    }
    expect(registry.actions()).toEqual([]);
  });

  it('derives_the_permission_of_a_method_backed_action_and_refuses_drift', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    // No `permission` restated: the answer comes from METHOD_PERMISSIONS.
    const dispose = registry.register(
      spec({
        name: 'jobs.cancel',
        method: { service: 'rpc.operations', method: 'cancelOperation' },
        permission: undefined,
        risk: 'write',
      }),
    );
    const action = registry.get('jobs.cancel');
    expect(action?.permission).toBe('operations:cancel');
    expect(() => dispose()).not.toThrow();

    // Restating it wrongly is drift between the registry and the buttons.
    expect(() =>
      registry.register(
        spec({
          name: 'jobs.cancel',
          method: { service: 'rpc.operations', method: 'cancelOperation' },
          permission: 'jobs:read',
          risk: 'write',
        }),
      ),
    ).toThrow(/needs operations:cancel/);
  });

  it('refuses_a_method_that_is_not_bridged', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    expect(() =>
      registry.register(
        spec({ name: 'jobs.cancel', method: { service: 'rpc.jobs', method: 'nuke' } }),
      ),
    ).toThrow(/rpc.jobs.nuke cannot be bridged/);
    expect(() =>
      registry.register(
        spec({ name: 'jobs.cancel', method: { service: 'session', method: 'principal' } }),
      ),
    ).toThrow(/never available to sandboxed plugins/);
  });

  it('lists_actions_in_registration_order_and_by_plugin', () => {
    const registry = registryWith(
      spec(),
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
      }),
    );
    expect(registry.actions().map((a) => a.name)).toEqual(['jobs.list', 'ops.drop']);
    expect(registry.byPlugin('ops').map((a) => a.name)).toEqual(['ops.drop']);
    expect(registry.get('nope')).toBeUndefined();
  });

  it('bumps_a_version_so_a_consumer_can_regenerate', () => {
    const registry = new ActionRegistry(ADMISSIONS);
    const before = registry.getVersion();
    const dispose = registry.register(spec());
    expect(registry.getVersion()).toBeGreaterThan(before);
    const after = registry.getVersion();
    dispose();
    expect(registry.getVersion()).toBeGreaterThan(after);
  });
});

describe('invoking an action', () => {
  it('runs_a_permitted_read_action', async () => {
    const registry = registryWith(spec());
    const result = await registry.invoke('jobs.list', undefined, call());
    expect(result).toEqual({ ok: true, value: ['default', 'retry'] });
  });

  it('refuses_input_it_cannot_read_instead_of_rejecting', async () => {
    // Input arrives from a page, so it can be a Proxy whose own traps throw.
    // `invoke` resolves an `ActionResult` and never rejects: a rejection would
    // cross into the WebMCP layer as an opaque `UnknownError`, taking the
    // reason with it. So a throw while *inspecting* the input is a refusal
    // like any other.
    const hostile = new Proxy(
      {},
      {
        // Satisfies `required` so validation reaches `Object.entries`, then
        // throws there — the point where a bare inspection would escape.
        get: (_target, key) => (key === 'queue' ? 'critical' : undefined),
        ownKeys() {
          throw new Error('keys are not for you');
        },
      },
    );
    const registry = registryWith(
      spec({
        inputSchema: {
          type: 'object',
          properties: { queue: { type: 'string' } },
          required: ['queue'],
        },
      }),
    );
    const result = await registry.invoke('jobs.list', hostile, call());
    expect(result.ok).toBe(false);
    expect(result.ok === false && result.error.code).toBe('invalid_input');
  });

  it('refuses_an_unknown_action', async () => {
    const registry = registryWith(spec());
    const result = await registry.invoke('jobs.purge', undefined, call());
    expect(result).toEqual({
      ok: false,
      error: { code: 'unknown_action', message: 'jobs.purge is not a registered action' },
    });
  });

  it('refuses_when_the_policy_does_not_carry_the_permission', async () => {
    // The registry's own admission decides, exactly as the bridge does.
    const registry = new ActionRegistry({
      jobs: { actions: ['jobs.list'], policy: { services: ['rpc.jobs'], permissions: [] } },
    });
    registry.register(spec());
    const result = await registry.invoke('jobs.list', undefined, call());
    expect(result).toEqual({
      ok: false,
      error: { code: 'permission_denied', message: 'jobs.jobs.list needs jobs:read' },
    });
  });

  it('refuses_a_method_backed_action_whose_service_is_not_injected', async () => {
    // The permission is granted but the plugin does not inject the service, so
    // the call is the one the bridge would refuse: `decideCall` decides both,
    // and the registry asks it rather than checking only half.
    const registry = new ActionRegistry({
      jobs: {
        actions: ['jobs.cancel'],
        policy: { services: [], permissions: ['operations:cancel'] },
      },
    });
    let ran = 0;
    registry.register(
      spec({
        name: 'jobs.cancel',
        method: { service: 'rpc.operations', method: 'cancelOperation' },
        // Derived from METHOD_PERMISSIONS; restating it would be drift.
        permission: undefined,
        risk: 'write',
        execute: async () => {
          ran++;
          return 'cancelled';
        },
      }),
    );
    const result = await registry.invoke('jobs.cancel', undefined, call());
    expect(result).toEqual({
      ok: false,
      error: {
        code: 'permission_denied',
        message: "rpc.operations is not in the plugin's inject list",
      },
    });
    expect(ran).toBe(0);
  });

  it('refuses_a_destructive_action_when_no_gate_is_wired', async () => {
    // §39 §8 rule 1: `destructive` is approve. With no way to obtain a real
    // decision the honest outcome is a refusal, never an auto-approval.
    let ran = 0;
    const registry = registryWith(
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
        execute: async () => {
          ran++;
          return 'dropped';
        },
      }),
    );
    const result = await registry.invoke('ops.drop', undefined, call());
    expect(result).toEqual({
      ok: false,
      error: {
        code: 'approval_required',
        message: 'ops.drop is destructive and no approval gate is wired, so it is refused',
      },
    });
    expect(ran).toBe(0);
  });

  it('asks_the_gate_and_runs_only_after_another_person_approved', async () => {
    let ran = 0;
    const registry = registryWith(
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
        execute: async () => {
          ran++;
          return 'dropped';
        },
      }),
    );
    const seen: unknown[] = [];
    const result = await registry.invoke(
      'ops.drop',
      { reason: 'stale queue' },
      call({
        approval: async (request) => {
          seen.push(request);
          return { approved: true, decidedBy: 'usr_omar', approvalId: 'apr_1' };
        },
      }),
    );
    expect(result).toEqual({ ok: true, value: 'dropped' });
    expect(ran).toBe(1);
    expect(seen).toEqual([
      {
        action: 'ops.drop',
        plugin: 'ops',
        risk: 'destructive',
        summary: 'List the job queues.',
        requester: 'agent:webmcp',
        input: { reason: 'stale queue' },
      },
    ]);
  });

  it('refuses_when_the_requester_tried_to_approve_their_own_request', async () => {
    // §39 §8 rule 2, and the check a client can honestly make.
    let ran = 0;
    const registry = registryWith(
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
        execute: async () => {
          ran++;
          return 'dropped';
        },
      }),
    );
    const result = await registry.invoke(
      'ops.drop',
      undefined,
      call({
        approval: async () => ({ approved: true, decidedBy: 'agent:webmcp', approvalId: 'apr_1' }),
      }),
    );
    expect(result).toEqual({
      ok: false,
      error: {
        code: 'approval_denied',
        message: 'ops.drop: agent:webmcp cannot approve their own request',
      },
    });
    expect(ran).toBe(0);
  });

  it('refuses_when_the_gate_itself_fails', async () => {
    // A gate is outside our control (it awaits a person, or a broker). If it
    // throws, the honest outcome is still a resolved refusal: `invoke` does
    // not reject, so a rejection here would be the one path that could.
    let ran = 0;
    const registry = registryWith(
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
        execute: async () => {
          ran++;
          return 'dropped';
        },
      }),
    );
    const result = await registry.invoke(
      'ops.drop',
      undefined,
      call({
        approval: async () => {
          throw new Error('the broker is unreachable');
        },
      }),
    );
    expect(result).toEqual({
      ok: false,
      error: {
        code: 'approval_denied',
        message: 'ops.drop: the approval gate failed: the broker is unreachable',
      },
    });
    expect(ran).toBe(0);
  });

  it('refuses_a_malformed_approval_decision', async () => {
    // A decision without a decider cannot be checked against rule 2, and one
    // without an approval id cannot be checked by the broker, so neither is
    // treated as an approval.
    const registry = registryWith(
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
      }),
    );
    for (const decision of [
      { approved: true, decidedBy: '', approvalId: 'apr_1' },
      { approved: true, decidedBy: 'usr_omar', approvalId: '' },
      undefined,
      'yes',
    ]) {
      const result = await registry.invoke(
        'ops.drop',
        undefined,
        call({
          approval: async () => decision as never,
        }),
      );
      expect(result.ok, JSON.stringify(decision)).toBe(false);
      expect(result.ok === false && result.error.code).toBe('approval_denied');
    }
  });

  it('hands_the_approval_proof_to_execute_so_the_broker_can_verify_it', async () => {
    // §39 §8 rule 2: "a destructive call carries the approval id and the broker
    // verifies it is settled and matches the call's hash". So the id cannot just
    // be checked and dropped — `execute` is what makes the call, and it is the
    // only thing that can carry the proof to the credential broker.
    const seen: unknown[] = [];
    const registry = registryWith(
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
        execute: async (_input, context) => {
          seen.push(context);
          return 'dropped';
        },
      }),
    );
    const result = await registry.invoke(
      'ops.drop',
      undefined,
      call({
        approval: async () => ({ approved: true, decidedBy: 'usr_omar', approvalId: 'apr_7' }),
      }),
    );
    expect(result.ok).toBe(true);
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({
      approval: { approvalId: 'apr_7', decidedBy: 'usr_omar' },
    });
  });

  it('carries_no_approval_proof_when_no_gate_was_needed', async () => {
    // A read needs no gate, so `execute` must not receive a proof-shaped object
    // it might present as one.
    let seen: Record<string, unknown> | undefined;
    const registry = registryWith(
      spec({
        execute: async (_input, context) => {
          seen = context as unknown as Record<string, unknown>;
          return 'ok';
        },
      }),
    );
    await registry.invoke('jobs.list', undefined, call());
    expect(seen?.approvalId).toBeUndefined();
    expect(seen?.decidedBy).toBeUndefined();
  });

  it('refuses_when_the_gate_rejects', async () => {
    const registry = registryWith(
      spec({
        name: 'ops.drop',
        plugin: 'ops',
        permission: 'operations:cancel',
        risk: 'destructive',
      }),
    );
    const result = await registry.invoke(
      'ops.drop',
      undefined,
      call({
        approval: async () => ({ approved: false, reason: 'the person said no' }),
      }),
    );
    expect(result).toEqual({
      ok: false,
      error: { code: 'approval_denied', message: 'ops.drop: the person said no' },
    });
  });

  it('gates_a_write_in_a_protected_environment_but_not_in_an_open_one', async () => {
    // §39 §8 rule 1: write is allow outside protected environments.
    const registry = registryWith(
      spec({ name: 'jobs.cancel', risk: 'write', permission: 'operations:cancel' }),
    );
    const open = await registry.invoke(
      'jobs.cancel',
      undefined,
      call({ environment: { id: 'env_dev', protected: false } }),
    );
    expect(open.ok).toBe(true);

    const asked: string[] = [];
    const protectedRun = await registry.invoke(
      'jobs.cancel',
      undefined,
      call({
        environment: { protected: true },
        approval: async (request) => {
          asked.push(request.action);
          return { approved: true, decidedBy: 'usr_omar', approvalId: 'apr_2' };
        },
      }),
    );
    expect(asked).toEqual(['jobs.cancel']);
    expect(protectedRun.ok).toBe(true);
  });

  it('gates_a_write_whose_environment_is_unknown', async () => {
    // `write` is allowed outside a protected environment — but "outside one" is a
    // claim about a named environment. A call that named none has not made it, so
    // treating an absent `environment` as unprotected is fail-open on the same
    // field the gate reads. §39 §8 rule 1's `write` default is "allow in
    // non-protected environments", and an unnamed environment is not known to be
    // one. The conservative reading is the honest one for a surface an agent
    // drives: gate it.
    const registry = registryWith(spec({ name: 'jobs.cancel', risk: 'write' }));
    const unnamed = await registry.invoke('jobs.cancel', undefined, call());
    expect(unnamed.ok === false && unnamed.error.code).toBe('approval_required');

    // Explicitly unprotected is the claim §39 §8 rule 1 accepts, so it runs.
    const open = await registry.invoke(
      'jobs.cancel',
      undefined,
      call({ environment: { id: 'env_dev', protected: false } }),
    );
    expect(open.ok).toBe(true);
  });

  it('refuses_a_write_in_a_protected_environment_with_no_gate', async () => {
    const registry = registryWith(
      spec({ name: 'jobs.cancel', risk: 'write', permission: 'operations:cancel' }),
    );
    const result = await registry.invoke(
      'jobs.cancel',
      undefined,
      call({ environment: { protected: true } }),
    );
    expect(result.ok).toBe(false);
    expect(result.ok === false && result.error.code).toBe('approval_required');
  });

  it('validates_the_input_against_the_declared_schema', async () => {
    const registry = registryWith(
      spec({
        inputSchema: {
          type: 'object',
          properties: { name: { type: 'string' }, limit: { type: 'integer' } },
          required: ['name'],
          additionalProperties: false,
        },
      }),
    );
    const missing = await registry.invoke('jobs.list', { limit: 2 }, call());
    expect(missing.ok === false && missing.error.code).toBe('invalid_input');
    expect(missing.ok === false && missing.error.message).toMatch(/name/);

    const extra = await registry.invoke('jobs.list', { name: 'a', other: 1 }, call());
    expect(extra.ok === false && extra.error.message).toMatch(/other/);

    const wrongType = await registry.invoke('jobs.list', { name: 'a', limit: 1.5 }, call());
    expect(wrongType.ok === false && wrongType.error.message).toMatch(/limit/);

    const ok = await registry.invoke('jobs.list', { name: 'a', limit: 2 }, call());
    expect(ok.ok).toBe(true);
  });

  it('refuses_input_that_is_not_a_plain_object', async () => {
    const registry = registryWith(spec());
    for (const input of [null, 7, 'x', ['a'], new Date(0)]) {
      const result = await registry.invoke('jobs.list', input, call());
      expect(result.ok === false && result.error.code, String(input)).toBe('invalid_input');
    }
  });

  it('turns_a_throwing_execute_into_a_resolved_refusal', async () => {
    // The WebMCP draft reports a rejected `execute` as an opaque UnknownError,
    // so an honest code and message have to travel in the resolved value.
    const registry = registryWith(
      spec({
        execute: () => {
          throw new Error('the queue is gone');
        },
      }),
    );
    const result = await registry.invoke('jobs.list', undefined, call());
    expect(result).toEqual({
      ok: false,
      error: { code: 'failed', message: 'jobs.list: the queue is gone' },
    });
  });

  it('refuses_when_the_signal_is_already_aborted', async () => {
    let ran = 0;
    const registry = registryWith(
      spec({
        execute: async () => {
          ran++;
          return 'x';
        },
      }),
    );
    const result = await registry.invoke(
      'jobs.list',
      undefined,
      call({ signal: AbortSignal.abort() }),
    );
    expect(result.ok === false && result.error.code).toBe('aborted');
    expect(ran).toBe(0);
  });

  it('passes_the_signal_and_the_environment_to_execute', async () => {
    const seen: unknown[] = [];
    const registry = registryWith(
      spec({
        execute: async (_input, context) => {
          seen.push(context);
          return 'ok';
        },
      }),
    );
    const signal = new AbortController().signal;
    await registry.invoke(
      'jobs.list',
      undefined,
      call({ signal, environment: { id: 'env_1', protected: false } }),
    );
    expect(seen).toEqual([
      { signal, environment: { id: 'env_1', protected: false }, requester: 'agent:webmcp' },
    ]);
  });
});

describe('the manifest actions list', () => {
  const pkg = (plugin: Record<string, unknown>) => ({
    name: '@loams/plugin-jobs',
    version: '0.1.0',
    loams: {
      plugin: { kind: 'console', entry: '.', tier: 'first-party', inject: [], ...plugin },
    },
  });

  it('accepts_a_list_of_action_names', () => {
    const manifest = validateManifest(pkg({ actions: ['jobs.list', 'jobs.cancel'] }));
    expect(manifest.actions).toEqual(['jobs.list', 'jobs.cancel']);
  });

  it('defaults_to_none_and_refuses_a_malformed_list', () => {
    expect(validateManifest(pkg({})).actions).toEqual([]);
    expect(() => validateManifest(pkg({ actions: 'jobs.list' }))).toThrow(ManifestError);
    expect(() => validateManifest(pkg({ actions: ['jobs list'] }))).toThrow(/actions/);
  });
});
