// The console's WebMCP surface (AP1d Task 1; design §42 §5, D568–D570, D635).
//
// A page registers tools with `document.modelContext`, so an in-browser agent
// calls a function instead of driving the UI. That is an *enhancement*: D568
// requires the console to work identically with the API absent, and D635 makes
// that load-bearing rather than prudent — WebKit's standards position on
// WebMCP is closed and `oppose`, so there is no version of this plan in which
// Safari is assumed to arrive.
//
// Task 1 was detection and the registration seam. Task 2 adds the generation:
// one tool per row of the plugin action registry (`@loams/console-host`,
// D569), so this module knows a tool's *shape* and the registry knows its
// meaning. The stdio MCP server (§30, D289) reads the same rows, which is why
// nothing here re-describes an action.
//
// The enforcement below is the reason this file is small. Every generated
// `execute` is `registry.invoke`, so a generated tool cannot be more permissive
// than the button behind it: the same `decideCall` / `PERMISSIONS` primitives
// decide, and a destructive action is refused unless a gate returns a decision
// from someone other than the requester (§39 §8 rules 1 and 2, D636). Nothing
// here can approve, and nothing here calls the network.
//
// The constraints below are not defensive coding. Each is a rule read off the
// spec's own IDL in Task 0 and recorded in the design, and each one is a way a
// naive implementation fails:
//
// - Registration-time `signal` **unregisters**; it does **not** cancel a call
//   already running. So sign-out needs one abort per registered tool, not one
//   global flag. `WebMcpTools.clear()` keeps a controller per tool for that
//   reason.
// - `exposedTo` entries must each be a *potentially trustworthy* origin, or the
//   whole `registerTool` promise rejects with `SecurityError`. A plaintext
//   `http://` agent origin cannot be listed at all, so a bad entry is a
//   registration failure rather than a silently dropped origin.
// - Access is gated by the `tools` Permissions Policy (default allowlist
//   `['self']`) and rejects with `NotAllowedError` otherwise.
// - A bare registration is same-origin only. That is the secure default, so
//   `exposedTo` is only ever set when the console is deliberately pairing with
//   another origin.
//
// `requestUserInteraction()` is deliberately absent: Chrome's security
// documentation mentions it, but it does not exist in the 2026-10-02 draft, and
// writing against it would be writing against a proposal.

import type { ActionInputSchema, ApprovalGate, Risk } from '@loams/console-host';

/**
 * A tool as the console hands it to the browser. Mirrors `ModelContextTool`:
 * `name`, `description` and `execute` are required by the spec; `title`,
 * `inputSchema` and `annotations` are optional.
 */
export interface WebMcpTool {
  name: string;
  description: string;
  execute(
    input: Record<string, unknown>,
    options: { signal: AbortSignal },
  ): Promise<unknown> | unknown;
  title?: string;
  inputSchema?: Record<string, unknown>;
  annotations?: {
    readOnlyHint?: boolean;
    untrustedContentHint?: boolean;
    consequentialHint?: boolean;
    debugging?: boolean;
  };
}

/**
 * The subset of `ModelContext` this module uses. Declared structurally rather
 * than pulled from a lib, so a build never depends on the spec having shipped
 * typings, and so a test can supply a fake with only these four members.
 */
export interface ModelContextLike {
  registerTool(
    tool: WebMcpTool,
    options?: { exposedTo?: string[]; signal?: AbortSignal },
  ): Promise<undefined>;
  getTools(options?: { fromOrigins?: string[] }): Promise<readonly { name: string }[]>;
  executeTool(
    tool: unknown,
    inputObject?: object,
    options?: { signal?: AbortSignal },
  ): Promise<string>;
  addEventListener(type: string, listener: () => void): void;
}

/**
 * Whether `document.modelContext` is present, and whether this document may use
 * it. Two separate conditions, because they fail differently: the attribute is
 * `[SecureContext]` (absent over plaintext http) and access is further gated by
 * the `tools` Permissions Policy (default `['self']`).
 *
 * Returns a reason rather than a bare boolean so the console can explain a
 * missing capability instead of silently having no tools — which, per D635, is
 * the normal state on Safari rather than an error.
 */
export function detectModelContext(doc: Document | undefined = globalThis.document): Detection {
  if (!doc) return { available: false, reason: 'no-document' };
  const context = (doc as Document & { modelContext?: ModelContextLike }).modelContext;
  if (!context) return { available: false, reason: 'not-exposed' };
  if (!isAllowedByPermissionsPolicy(doc)) {
    return { available: false, reason: 'blocked-by-permissions-policy' };
  }
  return { available: true, context };
}

export type Detection =
  | { available: true; context: ModelContextLike }
  | { available: false; reason: 'no-document' | 'not-exposed' | 'blocked-by-permissions-policy' };

/**
 * Whether the `tools` policy-controlled feature is allowed here.
 *
 * `document.permissionsPolicy` is not in every engine's DOM typings, so it is
 * read defensively. An engine with no `allowedFeatures` map cannot answer, and
 * an unknown engine is not evidence of a block — `registerTool` is the
 * authority and reports `NotAllowedError` itself if the policy really denies it.
 */
function isAllowedByPermissionsPolicy(doc: Document): boolean {
  const features = (
    doc as Document & {
      permissionsPolicy?: { allowedFeatures?: Readonly<Record<string, boolean>> };
    }
  ).permissionsPolicy?.allowedFeatures;
  if (!features) return true;
  return features.tools !== false;
}

/**
 * Whether an origin may be listed in `exposedTo`.
 *
 * The spec says "potentially trustworthy", which is the Secure Contexts
 * definition: `https`, `wss`, `file`, `localhost` and loopback literals are
 * trustworthy; a plaintext `http://` origin is not and the registration would
 * reject. Checking here turns a rejected promise into a skipped origin with a
 * reason.
 */
export function isTrustworthyOrigin(origin: string): boolean {
  let url: URL;
  try {
    url = new URL(origin);
  } catch {
    return false;
  }
  if (url.protocol === 'https:' || url.protocol === 'wss:' || url.protocol === 'file:') return true;
  if (url.protocol !== 'http:') return false;
  // http is trustworthy only for a potentially-local host.
  const host = url.hostname.replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host.endsWith('.localhost')) return true;
  if (/^127(?:\.\d{1,3}){3}$/.test(host)) return true;
  if (host === '::1') return true;
  return false;
}

/**
 * The origins a registration should be exposed to: the console's own origin
 * plus whichever of `agents` are trustworthy.
 *
 * Untrustworthy entries are dropped rather than passed through, because one bad
 * entry rejects the whole `registerTool` promise — dropping is recoverable and
 * passing it through loses every tool, not just that agent.
 */
export function exposedOrigins(
  location: { origin: string } | undefined = globalThis.location,
  agents: readonly string[] = [],
): { exposedTo: string[]; rejected: string[] } {
  const own = location?.origin;
  const exposedTo = own && isTrustworthyOrigin(own) ? [own] : [];
  const rejected: string[] = [];
  for (const agent of agents) {
    if (isTrustworthyOrigin(agent)) exposedTo.push(agent);
    else rejected.push(agent);
  }
  return { exposedTo, rejected };
}

/**
 * The subset of the action registry this module reads. Declared structurally so
 * the console does not need the host's whole class to generate a tool, and so
 * a test can pass a plain object.
 */
export interface ActionSource {
  actions(): readonly {
    name: string;
    description: string;
    title?: string;
    risk?: Risk;
    inputSchema?: ActionInputSchema;
  }[];
  invoke(
    name: string,
    input: unknown,
    options: {
      signal: AbortSignal;
      approval?: ApprovalGate;
      environment?: { id?: string; protected?: boolean };
      requester?: string;
    },
  ): Promise<unknown>;
}

export type { ApprovalGate } from '@loams/console-host';

/**
 * Generates one tool per registered action (D569).
 *
 * Two deliberate choices, both about what an agent can read before it calls:
 *
 * - `untrustedContentHint` is **always true**. The input is an agent's, so it
 *   is untrusted data (§39 §8 rule 5: text from apps is never a basis for a
 *   tool choice on its own). It is the one hint that does not vary per action.
 * - `readOnlyHint` follows the action's `risk` rather than its description, so
 *   a harness can filter on the annotation rather than parse prose.
 *
 * `execute` resolves whatever `invoke` resolves and never rejects, because the
 * draft turns a rejection into an opaque `UnknownError` (see the module header).
 * The registry's job, not this function's: the tool does not inspect, retry,
 * cache or re-interpret a refusal.
 */
export function webmcpToolsFrom(
  registry: ActionSource,
  options: {
    approval?: ApprovalGate;
    /**
     * The environment these tools act in. Bound at *generation*, by the host,
     * rather than read per call: an agent that could name its own environment
     * would be naming its own `protected: false`, which is the requester
     * approving its own request (§39 §8 rule 2). The registry gates a `write`
     * whose environment was never resolved to be unprotected, so this is what
     * lets a generated write run at all.
     */
    environment?: { id?: string; protected?: boolean };
  } = {},
): WebMcpTool[] {
  return registry.actions().map((action) => ({
    name: action.name,
    description: action.description,
    ...(action.title === undefined ? {} : { title: action.title }),
    ...(action.inputSchema === undefined ? {} : { inputSchema: action.inputSchema }),
    annotations: {
      readOnlyHint: (action.risk ?? 'read') === 'read',
      consequentialHint: (action.risk ?? 'read') !== 'read',
      untrustedContentHint: true,
    },
    // `input` is passed through as the browser gave it, including `undefined`
    // (a legitimate "no arguments", which the registry reads as `{}`) and an
    // explicit `null` (a wrong payload, which the registry refuses). Coercing
    // either here would launder a bad call into a good-looking one.
    execute: (input: Record<string, unknown>, options_: { signal: AbortSignal }) =>
      registry.invoke(action.name, input, {
        // Only the signal is read from the per-call options, and only ever the
        // signal: everything else an agent could put here — an approval gate, an
        // environment — would be the requester deciding its own case.
        signal: options_.signal,
        ...(options.approval ? { approval: options.approval } : {}),
        ...(options.environment ? { environment: options.environment } : {}),
      }),
  }));
}

/**
 * A registration handle: which tools are live, and the per-tool abort handles
 * that sign-out needs.
 *
 * `clear()` aborts every controller, which makes the spec unregister each tool.
 * The registration-time signal is per tool precisely so that one global
 * teardown cannot stand in for this.
 */
export class WebMcpTools {
  readonly context: ModelContextLike;
  readonly exposedTo: string[];
  readonly rejected: readonly string[];
  readonly errors: readonly string[];

  #controllers = new Map<string, AbortController>();
  #tools: WebMcpTool[] = [];

  private constructor(
    context: ModelContextLike,
    exposedTo: string[],
    rejected: readonly string[],
    errors: readonly string[],
  ) {
    this.context = context;
    this.exposedTo = exposedTo;
    this.rejected = rejected;
    this.errors = errors;
  }

  /** The tools currently registered. */
  get registered(): readonly WebMcpTool[] {
    return this.#tools;
  }

  /**
   * Registers `tools`, returning a handle, or `undefined` when the API is
   * absent — the D568 path, and the only normal one on Safari.
   *
   * Every registration is attempted even if one fails, so one malformed tool
   * does not cost the rest; the failures are collected on `errors` rather than
   * thrown, because there is no caller that could do anything useful with them.
   */
  static async register(
    tools: readonly WebMcpTool[],
    options: {
      agents?: readonly string[];
      doc?: Document;
      /**
       * The console's own origin. A parameter rather than a read of
       * `globalThis.location`, because it is a security input — it goes into
       * `exposedTo` — and a security input that a test cannot set is a security
       * input that is not tested.
       */
      location?: { origin: string };
    } = {},
  ): Promise<WebMcpTools | undefined> {
    const detection = detectModelContext(options.doc);
    if (!detection.available) return undefined;
    const { exposedTo, rejected } = exposedOrigins(
      options.location ?? globalThis.location,
      options.agents ?? [],
    );
    const errors: string[] = [];
    const handle = new WebMcpTools(detection.context, exposedTo, rejected, errors);
    for (const tool of tools) {
      const controller = new AbortController();
      try {
        await detection.context.registerTool(tool, { exposedTo, signal: controller.signal });
        handle.#controllers.set(tool.name, controller);
        handle.#tools.push(tool);
      } catch (cause) {
        errors.push(`${tool.name}: ${describe(cause)}`);
      }
    }
    return handle;
  }

  /** Unregisters every tool. Safe to call more than once. */
  clear(): void {
    for (const controller of this.#controllers.values()) controller.abort();
    this.#controllers.clear();
    this.#tools = [];
  }
}

function describe(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  return String(cause);
}
