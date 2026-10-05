import { expect, it, vi } from 'vitest';
import {
  detectModelContext,
  exposedOrigins,
  isTrustworthyOrigin,
  type ModelContextLike,
  type WebMcpTool,
  WebMcpTools,
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
