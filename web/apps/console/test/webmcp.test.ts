import { ActionRegistry, type ActionSpec } from '@loams/console-host';
import { expect, it, vi } from 'vitest';
import {
  detectModelContext,
  exposedOrigins,
  isTrustworthyOrigin,
  type ModelContextLike,
  type WebMcpTool,
  WebMcpTools,
  webmcpToolsFrom,
} from '../src/webmcp/index.js';

/** A model context that records what it was asked to register. */
function fakeContext(): ModelContextLike & {
  registered: { tool: WebMcpTool; options: unknown }[];
} {
  const registered: { tool: WebMcpTool; options: unknown }[] = [];
  return {
    registered,
    async registerTool(tool, options) {
      registered.push({ tool, options });
      return undefined;
    },
    async getTools() {
      return registered.map((r) => ({ name: r.tool.name }));
    },
    async executeTool() {
      return '';
    },
    addEventListener() {},
  };
}

/** A Document carrying only the members `detectModelContext` reads. */
function docWith(
  modelContext: ModelContextLike | undefined,
  allowedFeatures?: Record<string, boolean>,
): Document {
  return {
    modelContext,
    permissionsPolicy: allowedFeatures ? { allowedFeatures } : undefined,
  } as unknown as Document;
}

function tool(name: string): WebMcpTool {
  return { name, description: `tool ${name}`, execute: async () => name };
}

const origin = { origin: 'https://console.example' };

// --- detection -------------------------------------------------------------

it('detects_the_model_context_when_the_document_exposes_it', () => {
  const context = fakeContext();
  const detection = detectModelContext(docWith(context));
  expect(detection.available).toBe(true);
  expect(detection.available && detection.context).toBe(context);
});

it('reports_not_exposed_when_the_attribute_is_absent', () => {
  // The Safari case, and per D635 the normal one: the API is simply not there.
  const detection = detectModelContext(docWith(undefined));
  expect(detection).toEqual({ available: false, reason: 'not-exposed' });
});

it('reports_blocked_when_the_tools_policy_denies_it', () => {
  const detection = detectModelContext(docWith(fakeContext(), { tools: false }));
  expect(detection).toEqual({ available: false, reason: 'blocked-by-permissions-policy' });
});

it('does_not_block_when_the_policy_allows_tools_or_cannot_answer', () => {
  expect(detectModelContext(docWith(fakeContext(), { tools: true })).available).toBe(true);
  // An engine with no `allowedFeatures` map is not evidence of a block.
  expect(detectModelContext(docWith(fakeContext())).available).toBe(true);
});

it('reports_no_document_outside_a_browser', () => {
  // `undefined` means "use the global", because that is the ergonomic call a
  // caller makes, so the guard is exercised with an explicit absent document
  // rather than by omitting the argument.
  expect(detectModelContext(null as unknown as Document)).toEqual({
    available: false,
    reason: 'no-document',
  });
});

// --- exposedTo -------------------------------------------------------------

it('treats_https_wss_file_and_localhost_as_trustworthy', () => {
  for (const good of [
    'https://console.example',
    'https://console.example:8443',
    'wss://agent.example',
    'file:///srv/app',
    'http://localhost:5173',
    'http://app.localhost',
    'http://127.0.0.1:3000',
    'http://[::1]:3000',
  ]) {
    expect(isTrustworthyOrigin(good), good).toBe(true);
  }
});

it('treats_plaintext_http_and_garbage_as_untrustworthy', () => {
  for (const bad of [
    'http://agent.example',
    'http://192.168.1.10:3000',
    'ws://agent.example',
    'not a url',
    '',
  ]) {
    expect(isTrustworthyOrigin(bad), bad).toBe(false);
  }
});

it('includes_the_console_origin_and_only_trustworthy_agents', () => {
  const { exposedTo, rejected } = exposedOrigins(origin, [
    'https://agent.example',
    'http://insecure.example',
  ]);
  expect(exposedTo).toEqual(['https://console.example', 'https://agent.example']);
  expect(rejected).toEqual(['http://insecure.example']);
});

it('drops_an_untrustworthy_agent_rather_than_losing_every_tool', () => {
  // One bad entry rejects the whole registerTool promise, so it must not be
  // passed through: the cost of dropping is one agent, the cost of passing it
  // is every tool.
  const { exposedTo } = exposedOrigins(origin, ['http://insecure.example']);
  expect(exposedTo).toEqual(['https://console.example']);
});

// --- registration ----------------------------------------------------------

it('registers_nothing_and_returns_undefined_when_the_api_is_absent', async () => {
  const handle = await WebMcpTools.register([tool('a')], {
    doc: docWith(undefined),
    agents: [],
    location: origin,
  });
  expect(handle).toBeUndefined();
});

it('registers_each_tool_with_exposed_to_and_a_signal', async () => {
  const context = fakeContext();
  const handle = await WebMcpTools.register([tool('a'), tool('b')], {
    doc: docWith(context),
    agents: ['https://agent.example'],
    location: origin,
  });
  expect(handle?.registered.map((t) => t.name)).toEqual(['a', 'b']);
  expect(context.registered).toHaveLength(2);
  for (const call of context.registered) {
    expect(call.options).toMatchObject({
      exposedTo: ['https://console.example', 'https://agent.example'],
    });
    // The spec's registerTool options are `exposedTo` and `signal`; anything else
    // is not in the 2026-10-02 draft.
    expect(Object.keys(call.options as object).sort()).toEqual(['exposedTo', 'signal']);
  }
});

it('keeps_one_abort_per_tool_so_sign_out_can_unregister_each', async () => {
  const context = fakeContext();
  const signals: AbortSignal[] = [];
  const spy = {
    ...context,
    registerTool: async (
      toolArg: WebMcpTool,
      options?: { exposedTo?: string[]; signal?: AbortSignal },
    ) => {
      context.registered.push({ tool: toolArg, options });
      if (options?.signal) signals.push(options.signal);
      return undefined;
    },
  };
  const handle = await WebMcpTools.register([tool('a'), tool('b')], {
    doc: docWith(spy),
    agents: [],
    location: origin,
  });
  expect(signals).toHaveLength(2);
  expect(signals.every((s) => !s.aborted)).toBe(true);

  handle?.clear();
  // Each registration-time signal is aborted, which is what unregisters. A
  // single shared signal would only ever have been able to do this for one.
  expect(signals.every((s) => s.aborted)).toBe(true);
  expect(handle?.registered).toEqual([]);
});

it('registers_the_rest_when_one_tool_fails', async () => {
  const context = fakeContext();
  const failing: ModelContextLike = {
    ...context,
    async registerTool(toolArg, options) {
      if (toolArg.name === 'bad') throw new Error('SecurityError');
      return context.registerTool(toolArg, options);
    },
  };
  const handle = await WebMcpTools.register([tool('bad'), tool('good')], {
    doc: docWith(failing),
    agents: [],
    location: origin,
  });
  expect(handle?.registered.map((t) => t.name)).toEqual(['good']);
  expect(handle?.errors).toEqual(['bad: SecurityError']);
});

it('is_safe_to_clear_twice', async () => {
  const handle = await WebMcpTools.register([tool('a')], {
    doc: docWith(fakeContext()),
    agents: [],
    location: origin,
  });
  expect(() => {
    handle?.clear();
    handle?.clear();
  }).not.toThrow();
});

it('does_not_call_a_method_the_draft_does_not_define', async () => {
  // `requestUserInteraction()` appears in Chrome's security documentation but is
  // absent from the draft; a fake that throws on any unknown property catches
  // a call to it.
  const context = new Proxy(fakeContext(), {
    get(target, prop, receiver) {
      if (prop in target) return Reflect.get(target, prop, receiver);
      throw new Error(`unexpected member accessed: ${String(prop)}`);
    },
  }) as ModelContextLike;
  const handle = await WebMcpTools.register([tool('a')], { doc: docWith(context), agents: [] });
  expect(handle?.registered).toHaveLength(1);
});

it('registration_does_not_touch_the_network', async () => {
  const fetchSpy = vi.fn();
  vi.stubGlobal('fetch', fetchSpy);
  await WebMcpTools.register([tool('a')], {
    doc: docWith(fakeContext()),
    agents: [],
    location: origin,
  });
  expect(fetchSpy).not.toHaveBeenCalled();
  vi.unstubAllGlobals();
});

// --- tools generated from the plugin action registry (AP1d Task 2) ---------

const ADMISSIONS = {
  jobs: {
    actions: ['jobs.list', 'jobs.cancel'],
    policy: { services: ['rpc.jobs'], permissions: ['jobs:read', 'operations:cancel'] },
  },
  ops: {
    actions: ['ops.drop'],
    policy: { services: ['rpc.operations'], permissions: ['operations:cancel'] },
  },
};

function action(over: Partial<ActionSpec> = {}): ActionSpec {
  return {
    name: 'jobs.list',
    plugin: 'jobs',
    description: 'List the job queues.',
    risk: 'read',
    permission: 'jobs:read',
    execute: async () => ['default'],
    ...over,
  };
}

function registryWith(...specs: ActionSpec[]): ActionRegistry {
  const registry = new ActionRegistry(ADMISSIONS);
  for (const spec of specs) registry.register(spec);
  return registry;
}

const destructive = () =>
  action({
    name: 'ops.drop',
    plugin: 'ops',
    description: 'Drop the operation.',
    risk: 'destructive',
    permission: 'operations:cancel',
    execute: async () => 'dropped',
  });

const generated = (registry: ActionRegistry) => webmcpToolsFrom(registry);

it('generates_one_tool_per_action_with_its_name_description_and_schema', () => {
  const tools = generated(
    registryWith(
      action({ inputSchema: { type: 'object', properties: { name: { type: 'string' } } } }),
    ),
  );
  expect(tools.map((t) => t.name)).toEqual(['jobs.list']);
  expect(tools[0]?.description).toBe('List the job queues.');
  expect(tools[0]?.inputSchema).toEqual({
    type: 'object',
    properties: { name: { type: 'string' } },
  });
  // `readOnlyHint` follows the action's risk, so an agent can read it off the
  // annotation rather than guessing from the description.
  expect(tools[0]?.annotations).toEqual({
    readOnlyHint: true,
    consequentialHint: false,
    untrustedContentHint: true,
  });
});

it('marks_a_write_tool_consequential_and_a_destructive_one_too', () => {
  const tools = generated(
    registryWith(
      action({ name: 'jobs.cancel', risk: 'write', permission: 'operations:cancel' }),
      destructive(),
    ),
  );
  expect(tools.find((t) => t.name === 'jobs.cancel')?.annotations).toEqual({
    readOnlyHint: false,
    consequentialHint: true,
    untrustedContentHint: true,
  });
  expect(tools.find((t) => t.name === 'ops.drop')?.annotations).toEqual({
    readOnlyHint: false,
    consequentialHint: true,
    untrustedContentHint: true,
  });
});

it('a_generated_tool_refuses_a_destructive_action_it_cannot_approve', async () => {
  // §39 §8 rule 1 and D636: a destructive action is never auto-approved, and a
  // client-side implementation may only refuse. With no gate wired, the honest
  // outcome is a refusal in the resolved value — not a rejection, which the
  // draft would report as an opaque UnknownError.
  const tools = generated(registryWith(destructive()));
  const result = await tools[0]?.execute(
    { reason: 'stale' },
    { signal: new AbortController().signal },
  );
  expect(result).toEqual({
    ok: false,
    error: {
      code: 'approval_required',
      message: 'ops.drop is destructive and no approval gate is wired, so it is refused',
    },
  });
});

it('a_generated_destructive_tool_runs_once_another_person_approved', async () => {
  const registry = registryWith(destructive());
  const tools = webmcpToolsFrom(registry, {
    approval: async () => ({ approved: true, decidedBy: 'usr_omar', approvalId: 'apr_1' }),
  });
  const result = await tools[0]?.execute({}, { signal: new AbortController().signal });
  expect(result).toEqual({ ok: true, value: 'dropped' });
});

it('a_generated_tool_refuses_a_self_approved_destructive_call', async () => {
  const tools = webmcpToolsFrom(registryWith(destructive()), {
    approval: async () => ({ approved: true, decidedBy: 'agent:webmcp', approvalId: 'apr_1' }),
  });
  const result = await tools[0]?.execute({}, { signal: new AbortController().signal });
  expect(result).toEqual({
    ok: false,
    error: {
      code: 'approval_denied',
      message: 'ops.drop: agent:webmcp cannot approve their own request',
    },
  });
});

it('a_generated_tool_names_the_environment_it_ran_in', async () => {
  // The registry gates a `write` on the environment being *known* to be
  // unprotected, so a generated tool has to be able to say which one it ran in.
  // With no way to pass one, every generated write would be refused forever —
  // a tool the caller cannot satisfy is a tool nobody uses. So the environment
  // is an argument to `webmcpToolsFrom`, not a per-call parameter an agent
  // chooses (an agent choosing its own `protected: false` would be the agent
  // approving its own request, which is §39 §8 rule 2).
  const registry = registryWith(action({ name: 'jobs.cancel', risk: 'write' }));
  const tools = webmcpToolsFrom(registry, { environment: { id: 'env_dev', protected: false } });
  const result = await tools[0]?.execute({}, { signal: new AbortController().signal });
  expect(result).toEqual({ ok: true, value: ['default'] });
});

it('a_generated_tool_does_not_let_the_caller_choose_its_own_approval_gate', async () => {
  // An agent that could hand `execute` an approval gate would be handing itself
  // the §39 §8 decision. The gate is bound at generation, by the host, and the
  // per-call options carry only a signal.
  const registry = registryWith(destructive());
  const tools = generated(registry);
  await expect(
    tools[0]?.execute({}, {
      signal: new AbortController().signal,
      approval: async () => ({ approved: true, decidedBy: 'agent:webmcp', approvalId: 'apr_1' }),
    } as never),
  ).resolves.toMatchObject({ ok: false, error: { code: 'approval_required' } });
});

it('a_generated_tool_refuses_an_action_the_policy_does_not_allow', async () => {
  const registry = new ActionRegistry({
    jobs: { actions: ['jobs.list'], policy: { services: ['rpc.jobs'], permissions: [] } },
  });
  registry.register(action());
  const tools = generated(registry);
  const result = await tools[0]?.execute({}, { signal: new AbortController().signal });
  expect(result).toEqual({
    ok: false,
    error: { code: 'permission_denied', message: 'jobs.jobs.list needs jobs:read' },
  });
});

it('registers_no_tools_and_returns_undefined_with_the_api_absent', async () => {
  // The Safari case (D635): the actions exist, the surface does not, and the
  // console carries on without it.
  const context = fakeContext();
  const handle = await WebMcpTools.register(generated(registryWith(action(), destructive())), {
    doc: docWith(undefined),
    agents: [],
    location: origin,
  });
  expect(handle).toBeUndefined();
  expect(context.registered).toEqual([]);
});

it('registers_the_generated_tools_and_unregisters_each_on_sign_out', async () => {
  const context = fakeContext();
  const signals: AbortSignal[] = [];
  const spy = {
    ...context,
    registerTool: async (
      toolArg: WebMcpTool,
      options?: { exposedTo?: string[]; signal?: AbortSignal },
    ) => {
      context.registered.push({ tool: toolArg, options });
      if (options?.signal) signals.push(options.signal);
      return undefined;
    },
  };
  const handle = await WebMcpTools.register(generated(registryWith(action(), destructive())), {
    doc: docWith(spy),
    agents: ['https://agent.example'],
    location: origin,
  });
  expect(handle?.registered.map((t) => t.name)).toEqual(['jobs.list', 'ops.drop']);
  // One registration-time signal per tool: the spec's signal unregisters and
  // does not cancel a running call, so a shared one would only ever do this
  // for a single tool.
  expect(signals).toHaveLength(2);
  expect(signals.every((s) => !s.aborted)).toBe(true);
  handle?.clear();
  expect(signals.every((s) => s.aborted)).toBe(true);
});

it('a_generated_tool_refuses_a_null_input_rather_than_reading_it_as_empty', async () => {
  // The draft's `executeTool` may omit its input object entirely, which is
  // `undefined` and legitimately means "no arguments". An explicit `null` is a
  // wrong payload and must not be laundered into an empty one.
  const seen: unknown[] = [];
  const tools = generated(
    registryWith(
      action({
        execute: async (input) => {
          seen.push(input);
          return 'ok';
        },
      }),
    ),
  );
  const missing = await tools[0]?.execute(undefined as never, {
    signal: new AbortController().signal,
  });
  expect(missing).toEqual({ ok: true, value: 'ok' });
  expect(seen).toEqual([{}]);

  const explicitNull = await tools[0]?.execute(null as never, {
    signal: new AbortController().signal,
  });
  expect(explicitNull).toEqual({
    ok: false,
    error: { code: 'invalid_input', message: 'jobs.list: input must be a JSON object' },
  });
  expect(seen).toEqual([{}]);
});

it('a_generated_tool_refuses_rather_than_rejecting_on_a_failing_action', async () => {
  const tools = generated(
    registryWith(
      action({
        execute: () => {
          throw new Error('the queue is gone');
        },
      }),
    ),
  );
  await expect(tools[0]?.execute({}, { signal: new AbortController().signal })).resolves.toEqual({
    ok: false,
    error: { code: 'failed', message: 'jobs.list: the queue is gone' },
  });
});

it('generating_tools_does_not_call_a_method_the_draft_does_not_define', () => {
  const context = new Proxy(fakeContext(), {
    get(target, prop, receiver) {
      if (prop in target) return Reflect.get(target, prop, receiver);
      throw new Error(`unexpected member accessed: ${String(prop)}`);
    },
  }) as ModelContextLike;
  // Generation reads the registry, not the browser.
  expect(generated(registryWith(action()))).toHaveLength(1);
  expect(() => detectModelContext(docWith(context)).available).not.toThrow();
});
