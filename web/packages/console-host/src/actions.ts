// The plugin action registry (§42 §5, D569; D636).
//
// One description of each callable thing a plugin exposes: its name, its
// description, its input schema, the permission it needs and its execute
// closure. The console's WebMCP tools are generated from these rows (AP1d Task
// 2), and the stdio MCP server (§30, D289) is meant to read the same rows
// rather than restate them — which is why the name rule below is the
// intersection both consumers need (1–128 characters of `[A-Za-z0-9_.-]`) and
// not either one's private rule.
//
// Registration mirrors `SlotRegistry.register`: non-transactional, returning a
// disposer, so a plugin calls it inside `ctx.effect` and disposing the plugin
// removes the row. A refused registration throws and leaves every earlier row
// in place, which is what a plugin author debugging a bad manifest needs.
//
// **Enforcement is not re-implemented.** A method-backed action derives its
// permission from `METHOD_PERMISSIONS` through `decideCall` — the same
// primitive the sandbox bridge and the buttons go through — and restating it
// wrongly is refused at registration rather than silently trusted. A local
// action names a permission from `PERMISSIONS`, the same vocabulary the manifest
// declares. Either way this is a *shortening of the path to a refusal*: the
// server's OpenFGA check and the credential broker (§39 §6) remain the
// boundary, and this module can only ever refuse, never widen.
//
// **Approvals (D636).** §39 §8 rule 1 makes `destructive` an `approve`, and
// rule 2 says the gate is server-side in the credential broker: the requester
// cannot approve their own request. So an approval gate here is a *seam*, and
// the default is no gate at all, which means a destructive action is refused.
// A destructive action is never auto-approved, and the two checks this module
// can honestly make are that a decision exists and that its decider is not the
// requester. Everything else is the server's.
//
// Every failure is a *resolved* value, never a rejection: the WebMCP draft
// reports a rejected `execute` as an opaque `UnknownError`, which would throw
// away the code and message the caller needs in order to act.

import {
  type Decision,
  decideCall,
  isPermission,
  PERMISSIONS,
  type Permission,
} from './permissions.js';

/** §39 §8 rule 1's risk tags, carried on a skill's card and here on an action. */
export type Risk = 'read' | 'write' | 'destructive';

export const RISKS: readonly Risk[] = ['read', 'write', 'destructive'];

/**
 * A JSON Schema for an action's input, restricted to the subset the registry
 * validates. Deliberately small: an action's schema is *descriptive* (it tells
 * an agent and the stdio server what the input looks like), and the validation
 * here exists to refuse an obviously wrong call before `execute` sees it, not
 * to be a general JSON Schema implementation.
 */
export interface ActionInputSchema {
  type: 'object';
  properties?: Record<
    string,
    { type?: 'string' | 'number' | 'integer' | 'boolean' | 'array' | 'object' }
  >;
  required?: readonly string[];
  additionalProperties?: boolean;
  /**
   * A JSON Schema legitimately carries keywords this validator does not
   * interpret (`enum`, `minLength`, `$id`), and the whole object is handed to
   * the browser as the tool's `inputSchema`, so it stays open. Nothing here
   * reads an unlisted keyword.
   */
  [keyword: string]: unknown;
}

export interface ActionContext {
  /** The execution-time lifetime; aborted when the caller cancels the call. */
  signal: AbortSignal;
  /** The environment the call resolved to, when the caller named one. */
  environment?: { id?: string; protected?: boolean };
  /** Who is asking — the agent id the WebMCP call came from. */
  requester: string;
  /**
   * The approval the credential broker has to verify before it releases the
   * credential (§39 §8 rule 2: "a destructive call carries the approval id and
   * the broker verifies it is settled and matches the call's hash"). `execute`
   * is the only thing that makes the call, so it is the only thing that can
   * carry the proof — checking the id here and dropping it would leave the
   * broker nothing to verify. Absent when no gate was needed, which is why an
   * `execute` must never present it unless it is present.
   */
  approval?: { approvalId: string; decidedBy: string };
}

export interface ActionSpec {
  /** 1–128 characters of `[A-Za-z0-9_.-]`; unique across the registry. */
  name: string;
  /** The registering plugin's id; must have declared the name (§37 §5.3). */
  plugin: string;
  description: string;
  /**
   * §39 §8 rule 1's risk. **Required**, because it is the field the approval
   * gate reads: defaulting an absent one to `read` is a fail-*open* default on
   * the one declaration that decides whether a person is asked. §39 §8 rule 1
   * has the agent declare the risk, so a missing declaration is a refusal, not
   * a silent read.
   */
  risk: Risk;
  /**
   * The permission the action needs. Optional when `method` is given, because
   * then it is *derived* from `METHOD_PERMISSIONS`; required otherwise.
   */
  permission?: Permission;
  /** The Connect method this action wraps, if any. */
  method?: { service: string; method: string };
  inputSchema?: ActionInputSchema;
  title?: string;
  execute(input: Record<string, unknown>, context: ActionContext): Promise<unknown> | unknown;
}

/** What §39 §8 needs to put in front of the person who decides. */
export interface ApprovalRequest {
  action: string;
  plugin: string;
  risk: Risk;
  summary: string;
  requester: string;
  input: Record<string, unknown>;
}

/**
 * A decision from someone other than the requester, carrying the approval id
 * the credential broker verifies server-side (§39 §8 rule 2).
 */
export type ApprovalDecision =
  | { approved: true; decidedBy: string; approvalId: string }
  | { approved: false; reason: string };

export type ApprovalGate = (
  request: ApprovalRequest,
  signal: AbortSignal,
) => Promise<ApprovalDecision>;

export interface InvokeOptions {
  signal: AbortSignal;
  /**
   * The approval seam. Absent means "no way to obtain a decision", which
   * refuses rather than approves.
   */
  approval?: ApprovalGate;
  environment?: { id?: string; protected?: boolean };
  /** Who is asking; default `agent:webmcp`, the console's in-page agent. */
  requester?: string;
}

/** The refusal codes, stable so a caller can branch on them. */
export type ActionErrorCode =
  | 'unknown_action'
  | 'permission_denied'
  | 'approval_required'
  | 'approval_denied'
  | 'invalid_input'
  | 'aborted'
  | 'failed';

export type ActionResult =
  | { ok: true; value: unknown }
  | { ok: false; error: { code: ActionErrorCode; message: string } };

/** What the host knows about one plugin's callable surface. */
export interface Admission {
  /** The action names the plugin's manifest declares. */
  actions: readonly string[];
  /** `inject` and the permissions the host enforces, as `BridgePolicy` has them. */
  policy: { services: readonly string[]; permissions: readonly string[] };
}

export class ActionError extends Error {
  override name = 'ActionError';
}

const NAME = /^[A-Za-z0-9_.-]{1,128}$/;
const KNOWN_RISKS = new Set<string>(RISKS);

/** The default requester: a call that arrived through the page, not a person. */
export const DEFAULT_REQUESTER = 'agent:webmcp';

/**
 * The `actions` service: what plugins register, and the one place a call is
 * authorised. Constructed with the admissions the host derived from the
 * manifests and the grants, because a registry that could admit its own
 * authors would enforce nothing.
 */
export class ActionRegistry {
  #entries: { spec: ActionSpec; permission: Permission }[] = [];
  #version = 0;
  #listeners = new Set<() => void>();
  readonly #admissions: Record<string, Admission>;

  constructor(admissions: Record<string, Admission> = {}) {
    this.#admissions = admissions;
  }

  /**
   * Registers one action, returning its disposer. Throws `ActionError` on an
   * invalid or duplicate name, on a permission the vocabulary does not know,
   * on an action the plugin did not declare, and on a method whose permission
   * disagrees with the one the spec names.
   */
  register(spec: ActionSpec): () => void {
    const name = spec?.name;
    if (typeof name !== 'string' || !NAME.test(name)) {
      throw new ActionError(
        `action name must be 1-128 characters of [A-Za-z0-9_.-]; got ${JSON.stringify(name)}`,
      );
    }
    if (typeof spec.description !== 'string' || !spec.description.trim()) {
      throw new ActionError(`action ${name} needs a non-empty description`);
    }
    // Admission comes before the duplicate check, and deliberately so. "Is this
    // name taken" is a question about the registry's contents, and a plugin the
    // host never admitted has no business asking it: the difference between the
    // two refusals is a free oracle over what other plugins registered.
    const admission = admissionFor(this.#admissions, spec.plugin);
    if (!admission)
      throw new ActionError(`${String(spec.plugin)} is not admitted to register actions`);
    if (this.#entries.some((e) => e.spec.name === name)) {
      throw new ActionError(`action ${JSON.stringify(name)} is already registered`);
    }
    if (typeof spec.execute !== 'function') {
      throw new ActionError(`action ${name} needs an execute closure`);
    }
    if (!admission.actions.includes(name)) {
      throw new ActionError(`${spec.plugin} declares no action ${JSON.stringify(name)}`);
    }
    // The risk is checked before the permission, because a missing or unknown
    // one is the more dangerous answer: it decides whether anyone is asked.
    const risk = spec.risk;
    if (typeof risk !== 'string' || !KNOWN_RISKS.has(risk)) {
      throw new ActionError(
        `action ${name} must declare a risk of ${RISKS.join(', ')}; got ${JSON.stringify(spec.risk)}`,
      );
    }
    const permission = resolvePermission(spec);
    const row: { spec: ActionSpec; permission: Permission } = {
      spec: { ...spec, risk, permission },
      permission,
    };
    this.#entries = [...this.#entries, row];
    this.#changed();
    return () => {
      const before = this.#entries.length;
      this.#entries = this.#entries.filter((e) => e.spec !== row.spec);
      if (this.#entries.length !== before) this.#changed();
    };
  }

  /** Every action, in registration order. */
  actions(): ActionSpec[] {
    return this.#entries.map((e) => e.spec);
  }

  get(name: string): ActionSpec | undefined {
    return this.#entries.find((e) => e.spec.name === name)?.spec;
  }

  /** Every action one plugin registered (the diagnostics page). */
  byPlugin(plugin: string): ActionSpec[] {
    return this.#entries.filter((e) => e.spec.plugin === plugin).map((e) => e.spec);
  }

  /** For `useSyncExternalStore`: bumps on every change. */
  readonly getVersion = (): number => this.#version;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /**
   * Runs one action: permission, then input, then the approval gate, then
   * `execute`. Never rejects; every outcome is a resolved `ActionResult`.
   */
  async invoke(name: string, input: unknown, options: InvokeOptions): Promise<ActionResult> {
    const row = this.#entries.find((e) => e.spec.name === name);
    if (!row) {
      return {
        ok: false,
        error: { code: 'unknown_action', message: `${name} is not a registered action` },
      };
    }
    const { spec } = row;
    if (options.signal.aborted) return aborted(spec.name);

    const permission = decidePermission(spec, row.permission, this.#admissions);
    if (!permission.ok) return denied(permission);

    // Input is untrusted and can be a Proxy whose property access or enumeration
    // throws, so inspecting it is itself a failure mode. `invoke` resolves an
    // `ActionResult` and never rejects — a rejection would reach the WebMCP
    // layer as an opaque `UnknownError` and take the caller's ability to read
    // the reason with it — so a throw here is reported like any other refusal.
    let args: ReturnType<typeof validateInput>;
    try {
      args = validateInput(spec, input);
    } catch {
      // The message is fixed on purpose. Rendering the cause would mean calling
      // `String()` on an object that came from the page, and a trap on
      // `toString` would throw *here* — inside the handler meant to stop
      // `invoke` throwing — putting the rejection straight back.
      return {
        ok: false,
        error: {
          code: 'invalid_input',
          message: `${spec.name}: the input could not be read`,
        },
      };
    }
    if (!args.ok) return { ok: false, error: { code: 'invalid_input', message: args.message } };

    const requester = options.requester ?? DEFAULT_REQUESTER;
    const context: ActionContext = {
      signal: options.signal,
      environment: options.environment,
      requester,
    };
    if (needsApproval(spec.risk, options.environment)) {
      const gate = options.approval;
      if (!gate) {
        return {
          ok: false,
          error: {
            code: 'approval_required',
            message: gateRefusalMessage(spec, options.environment),
          },
        };
      }
      // The gate may await a person, so the call may be cancelled meanwhile.
      if (options.signal.aborted) return aborted(spec.name);
      const decision = await askGate(
        gate,
        {
          action: spec.name,
          plugin: spec.plugin,
          risk: spec.risk,
          summary: spec.description,
          requester,
          input: args.value,
        },
        options.signal,
      );
      if (!decision.ok) {
        return {
          ok: false,
          error: { code: 'approval_denied', message: `${spec.name}: ${decision.reason}` },
        };
      }
      // §39 §8 rule 2. The requester cannot approve their own request, and a
      // client cannot widen that into "cannot be the same person" — so this is
      // the strongest check available here, and the broker's the rest.
      if (decision.decidedBy === requester) {
        return {
          ok: false,
          error: {
            code: 'approval_denied',
            message: `${spec.name}: ${requester} cannot approve their own request`,
          },
        };
      }
      if (options.signal.aborted) return aborted(spec.name);
      // Carried, not discarded: the broker verifies this id, so `execute` is
      // the one that has to present it with the call it authorises.
      context.approval = { approvalId: decision.approvalId, decidedBy: decision.decidedBy };
    }

    try {
      return { ok: true, value: await spec.execute(args.value, context) };
    } catch (cause) {
      return {
        ok: false,
        error: {
          code: 'failed',
          message: `${spec.name}: ${cause instanceof Error ? cause.message : String(cause)}`,
        },
      };
    }
  }

  #changed(): void {
    this.#version++;
    for (const listener of this.#listeners) listener();
  }
}

/**
 * The permission an action needs.
 *
 * A method-backed action goes through `decideCall` with every permission
 * granted, which both *derives* the answer from `METHOD_PERMISSIONS` and
 * refuses a method that is not bridged at all. A named permission that
 * disagrees is drift between this registry and the buttons, and is refused at
 * registration rather than quietly trusted.
 */
function resolvePermission(spec: ActionSpec): Permission {
  if (spec.method) {
    const decision = decideCall(
      { services: [spec.method.service], permissions: PERMISSIONS },
      spec.method.service,
      spec.method.method,
    );
    if (!decision.ok) {
      throw new ActionError(`action ${spec.name}: ${decision.reason}`);
    }
    if (spec.permission && spec.permission !== decision.permission) {
      throw new ActionError(
        `action ${spec.name}: ${spec.method.service}.${spec.method.method} needs ${decision.permission}, not ${spec.permission}`,
      );
    }
    return decision.permission;
  }
  if (!spec.permission) {
    throw new ActionError(
      `action ${spec.name} names no permission; give it one or a "method" to derive it from`,
    );
  }
  if (!isPermission(spec.permission)) {
    throw new ActionError(
      `action ${spec.name}: unknown permission ${JSON.stringify(spec.permission)}`,
    );
  }
  return spec.permission;
}

/**
 * Whether the plugin's admitted policy lets this call happen.
 *
 * A method-backed action goes to `decideCall`, which checks *both* halves a
 * bridged call needs — the service is in the plugin's `inject` list and the
 * permission is held. A local action has no service, so only the permission
 * applies. In both cases the answer is the same one the bridge and the buttons
 * get, which is the whole point: a generated tool cannot be more permissive
 * than the button behind it.
 */
function decidePermission(
  spec: ActionSpec,
  permission: Permission,
  admissions: Record<string, Admission>,
): Decision {
  const policy = admissionFor(admissions, spec.plugin)?.policy;
  if (!policy) {
    return { ok: false, reason: `${spec.plugin} is not admitted to register actions` };
  }
  if (spec.method) {
    return decideCall(policy, spec.method.service, spec.method.method);
  }
  if (!policy.permissions.includes(permission)) {
    return { ok: false, reason: `${spec.plugin}.${spec.name} needs ${permission}` };
  }
  return { ok: true, permission };
}

/**
 * §39 §8 rule 1's defaults: `read` allow; `write` allow outside a protected
 * environment and approve inside one; `destructive` approve everywhere.
 *
 * The one reading worth stating is the last case for `write`. Rule 1's default
 * is "allow **in non-protected environments**", which is a claim about a
 * *named* environment. A call that named none has not made that claim, so
 * treating an absent environment as unprotected would be fail-open on the same
 * field the gate reads — and the surface driving it is an agent that did not
 * choose to be conservative. So an environment that was never resolved is
 * gated, and the only way to run a `write` is to say which environment it is.
 * §39 §8 rule 1's `read` default is unconditional, so `read` is untouched.
 */
function needsApproval(risk: Risk, environment?: { protected?: boolean }): boolean {
  if (risk === 'destructive') return true;
  if (risk !== 'write') return false;
  return environment?.protected !== false;
}

/**
 * Calls the gate and normalises whatever comes back into a decision or a
 * reason to refuse.
 *
 * Two things are defended here, both because a gate is code we do not own: it
 * may **throw** (it awaits a person, or a broker), and `invoke` does not
 * reject, so an exception is turned into a refusal rather than becoming the one
 * path out of this module that does. And its **shape** is checked, because a
 * decision with no decider cannot be checked against §39 §8 rule 2 and one with
 * no approval id cannot be verified by the broker — so neither is an approval,
 * whatever it says.
 */
async function askGate(
  gate: ApprovalGate,
  request: ApprovalRequest,
  signal: AbortSignal,
): Promise<{ ok: true; decidedBy: string; approvalId: string } | { ok: false; reason: string }> {
  let decision: unknown;
  try {
    decision = await gate(request, signal);
  } catch (cause) {
    const reason = cause instanceof Error ? cause.message : String(cause);
    return { ok: false, reason: `the approval gate failed: ${reason}` };
  }
  if (typeof decision !== 'object' || decision === null) {
    return { ok: false, reason: 'the approval gate returned no decision' };
  }
  const { approved, decidedBy, approvalId, reason } = decision as Record<string, unknown>;
  if (approved !== true) {
    return {
      ok: false,
      reason:
        typeof reason === 'string' && reason ? reason : 'the approval gate returned no decision',
    };
  }
  if (
    typeof decidedBy !== 'string' ||
    !decidedBy ||
    typeof approvalId !== 'string' ||
    !approvalId
  ) {
    return {
      ok: false,
      reason: 'the approval is missing its decider or approval id, so it cannot be verified',
    };
  }
  return { ok: true, decidedBy, approvalId };
}

function gateRefusalMessage(spec: ActionSpec, environment?: { protected?: boolean }): string {
  const what =
    spec.risk === 'destructive'
      ? 'destructive'
      : environment?.protected === false
        ? 'a write'
        : 'a write in an environment that was not resolved to be unprotected';
  return `${spec.name} is ${what} and no approval gate is wired, so it is refused`;
}

/**
 * The admission for a plugin id, or `undefined`.
 *
 * `Object.hasOwn` rather than a plain read because the admissions are a plain
 * object: `admissions['toString']` answers `Object.prototype.toString`, which
 * is truthy, and the caller would then read `undefined.includes` and report a
 * `TypeError` instead of the admission refusal a caller can act on. A
 * prototype member is not an admission.
 */
function admissionFor(
  admissions: Record<string, Admission>,
  plugin: string,
): Admission | undefined {
  if (typeof plugin !== 'string' || plugin === '') return undefined;
  return Object.hasOwn(admissions, plugin) ? admissions[plugin] : undefined;
}

/**
 * A minimal check of the input against the declared schema: a plain object,
 * every `required` key present, every present key's declared type, and no
 * undeclared key when `additionalProperties` is false.
 *
 * `null`, an array, a Date and a number are all refused: an action that took
 * whatever arrived would be one `execute` away from a crash the caller cannot
 * read.
 */
function validateInput(
  spec: ActionSpec,
  input: unknown,
): { ok: true; value: Record<string, unknown> } | { ok: false; message: string } {
  // Only an *absent* input is an empty object, because `executeTool` may be
  // called with no input at all. An explicit `null` is a wrong payload, not an
  // absent one, and an action that took whatever arrived would be one bad call
  // away from a crash the caller cannot read.
  const value = input === undefined ? {} : input;
  // `typeof null === 'object'`, so the null test has to come first or the
  // prototype read below throws instead of refusing.
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return { ok: false, message: `${spec.name}: input must be a JSON object` };
  }
  // A class instance, a Date and a Map all pass `typeof`, but their properties
  // are not the action's input: `Object.entries` would see methods and a
  // private state. Only a plain object (or a null-prototype one, which is what
  // structured clone produces) is.
  if (!isPlainObject(value)) {
    return { ok: false, message: `${spec.name}: input must be a plain JSON object` };
  }
  const object = value as Record<string, unknown>;
  const schema = spec.inputSchema;
  if (!schema) return { ok: true, value: object };
  for (const key of schema.required ?? []) {
    if (object[key] === undefined) {
      return { ok: false, message: `${spec.name}: ${JSON.stringify(key)} is required` };
    }
  }
  for (const [key, value_] of Object.entries(object)) {
    const property = schema.properties?.[key];
    if (!property) {
      if (schema.additionalProperties === false) {
        return { ok: false, message: `${spec.name}: ${JSON.stringify(key)} is not in the schema` };
      }
      continue;
    }
    if (value_ === undefined) continue;
    const wrong = !matchesType(value_, property.type);
    if (wrong) {
      return {
        ok: false,
        message: `${spec.name}: ${JSON.stringify(key)} must be ${property.type ?? 'declared'}`,
      };
    }
  }
  return { ok: true, value: object };
}

function isPlainObject(value: object): boolean {
  try {
    const proto = Object.getPrototypeOf(value) as object | null;
    return proto === Object.prototype || proto === null;
  } catch {
    // A Proxy that refuses `getPrototypeOf` is not something to execute with.
    return false;
  }
}

function matchesType(value: unknown, type: string | undefined): boolean {
  switch (type) {
    case undefined:
      return true;
    case 'string':
      return typeof value === 'string';
    case 'number':
      return typeof value === 'number' && Number.isFinite(value);
    case 'integer':
      return typeof value === 'number' && Number.isInteger(value);
    case 'boolean':
      return typeof value === 'boolean';
    case 'array':
      return Array.isArray(value);
    case 'object':
      return typeof value === 'object' && value !== null && !Array.isArray(value);
    default:
      return false;
  }
}

function aborted(name: string): ActionResult {
  return { ok: false, error: { code: 'aborted', message: `${name}: the call was cancelled` } };
}

function denied(decision: Decision & { ok: false }): ActionResult {
  return { ok: false, error: { code: 'permission_denied', message: decision.reason } };
}
