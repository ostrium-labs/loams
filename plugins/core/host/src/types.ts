/**
 * The platform contract.
 *
 * Everything a plugin declares about itself lives in `PluginManifest`, and
 * everything the platform knows about a plugin at runtime lives in
 * `PluginStatus` (the manifest plus live state). The two are kept as separate
 * interfaces on purpose: a manifest is static, checked-in data that a plugin
 * author writes once, while a status is what the console renders right now.
 * Merging them would mean a UI could accidentally write `state`.
 *
 * Other packages code against these shapes, so the field names here are frozen.
 */

import type { Context, Service } from "cordis";
import type { RouteSpec } from "./router.js";
import type { AgentBus } from "./bus.js";
import type { HttpRouter } from "./router.js";

/** A capability an agent advertises, e.g. Zulip's `listChannels`. */
export interface PluginAgentSkill {
  id: string;
  name: string;
  description: string;
  tags?: string[];
  examples?: string[];
  inputModes?: string[];
  outputModes?: string[];
}

/** Static, author-supplied description of a plugin. */
export interface PluginManifest {
  id: string;
  name: string;
  description: string;
  version: string;
  category?: string;
  /** Route the plugin's own UI is served at, e.g. "/plugins/zulip". */
  uiPath: string;
  icon?: string;
  /** true => the console toggle is disabled and the plugin is never unloaded. */
  alwaysOn?: boolean;
  /** Console sort key. Lower sorts first. Defaults to 100. */
  order?: number;
  /** Enabled state for a plugin that has never been toggled before. */
  defaultEnabled?: boolean;
  /**
   * Auth scopes a caller must hold before this plugin's routes or skills answer.
   *
   * Declared on the manifest rather than in a config file so that a scope requirement is
   * versioned with the plugin that imposes it, and so authorization is read straight off
   * `PluginStatus` without a second registry to keep in sync.
   *
   * ENFORCED at request time by `PluginHost`, which wraps every route a plugin registers
   * in a guard derived from this field. The check is per request rather than per
   * enable, because a session created before a plugin was deployed cannot have been
   * granted that plugin's scopes; see `evaluateScopeGate`.
   *
   * DEVIATION FROM THE SF1 CONTRACT, deliberate. Every other field of this interface is
   * byte-for-byte the SF1 `PluginManifest`. This one field is an additive extension and
   * is kept because it is load-bearing, not decorative: `PluginHost._scopeGuard` builds a
   * per-route guard from it, `PluginRegistry` and the A2A layer both call
   * `evaluateScopeGate` with it, and `PluginStatus` exposes the `blocked` state and
   * `missingScopes` that the console renders from it. Dropping the field would delete
   * request-time scope enforcement for plugin routes and skills outright. Additive means
   * a host built against SF1 still accepts every manifest this repo produces; it just
   * ignores a scope requirement it has no implementation for, so such a manifest must
   * not be treated as enforced on that host.
   */
  requiredScopes?: string[];
  upstream?: { product: string; envPrefix?: string };
  agent?: { name: string; description: string; version: string; skills: PluginAgentSkill[] };
}

/** A manifest plus live state. This is what GET /api/plugins returns per plugin. */
export interface PluginStatus extends PluginManifest {
  enabled: boolean;
  /**
   * `blocked` means the plugin is loaded and healthy but the CURRENT SESSION lacks the
   * scopes in `requiredScopes`. It is a per-session view rather than a property of the
   * plugin, which is why `GET /api/plugins` recomputes it per request instead of storing
   * it -- one user being under-privileged must not appear to break the catalog for
   * everyone else.
   */
  state: "loaded" | "unloaded" | "error" | "blocked";
  /** Present when state === "error". */
  error?: string;
  /**
   * The `requiredScopes` this session does not hold. Populated exactly when
   * state === "blocked", so the console can name what is missing rather than saying
   * "access denied".
   */
  missingScopes?: string[];
  /** Epoch ms the plugin last loaded/unloaded. */
  changedAt?: number;
}

/* -------------------------------------------------------------------------- */
/* Agent bus                                                                  */
/* -------------------------------------------------------------------------- */

/** Internal typed envelope carried by {@link AgentBus}. */
export interface AgentMessage {
  /** Originating agent id. */
  from: string;
  /** Target agent id, or undefined for broadcast. */
  to?: string;
  /** Correlates a request with its reply. */
  messageId: string;
  /** Skill being invoked, e.g. "listChannels". */
  skill: string;
  /** Skill arguments. */
  params?: Record<string, unknown>;
  /** Text form for A2A interop. */
  text?: string;
  data?: unknown;
  timestamp: number;
}

/** What a skill handler is given besides its params. */
export interface AgentSkillContext {
  /** The cordis context the owning plugin is attached to. */
  ctx: Context;
  /** The message that triggered this invocation. */
  message: AgentMessage;
  /** Emits a progress artifact for multi-step work (A2A `artifacts`). */
  progress?: (name: string, data: unknown) => void;
}

export interface AgentSkillHandler {
  /** Must equal a `PluginAgentSkill.id` on the owning manifest. */
  id: string;
  description?: string;
  handle: (params: Record<string, unknown>, api: AgentSkillContext) => unknown;
}

/* -------------------------------------------------------------------------- */
/* Plugin loading                                                             */
/* -------------------------------------------------------------------------- */

/** Everything a loader is handed at load time, so it never reaches for globals. */
export interface PluginRuntime {
  id: string;
  manifest: PluginManifest;
  /** The root cordis context the host attaches services to. */
  ctx: Context;
  /** Routes registered here disappear when the plugin is unloaded. */
  router: HttpRouter;
  /** Skills subscribed here disappear when the plugin is unloaded. */
  bus: AgentBus;
}

/**
 * How a plugin gets loaded and unloaded.
 *
 * All three hooks are optional and all three are torn down on `disable`. A
 * plugin that only contributes an agent block needs none of them: the manifest
 * plus a `skills` hook is enough.
 */
export interface PluginLoader {
  /** Cordis service to attach to the root context while the plugin is enabled. */
  service?: new (ctx: Context, config?: any) => Service;
  /** Config forwarded to `service`. */
  config?: Record<string, unknown>;
  /**
   * Free-form attach hook for plugins that are not a cordis service.
   * A returned function is treated as a disposer.
   */
  attach?: (runtime: PluginRuntime) => void | (() => void);
  /** REST routes, registered while enabled and removed on unload. */
  routes?: (runtime: PluginRuntime) => RouteSpec[];
  /** A2A/bus skills, subscribed while enabled and unsubscribed on unload. */
  skills?: (runtime: PluginRuntime) => AgentSkillHandler[];
}

/* -------------------------------------------------------------------------- */
/* Google A2A wire format (the envelope on top of the bus)                    */
/* -------------------------------------------------------------------------- */

/** A2A protocol version this server speaks. */
export const A2A_PROTOCOL_VERSION = "0.3.0";

export interface A2AAgentSkill {
  id: string;
  name: string;
  description: string;
  tags?: string[];
  examples?: string[];
  inputModes?: string[];
  outputModes?: string[];
}

export interface A2AAgentInterface {
  url: string;
  transport: string;
}

export interface A2AAgentCard {
  protocolVersion: string;
  name: string;
  description: string;
  version: string;
  url: string;
  provider?: { organization: string; url: string };
  capabilities: {
    streaming: boolean;
    pushNotifications: boolean;
    stateTransitionHistory: boolean;
  };
  defaultInputModes: string[];
  defaultOutputModes: string[];
  skills: A2AAgentSkill[];
  additionalInterfaces?: A2AAgentInterface[];
}

export interface A2ATextPart {
  kind: "text";
  text: string;
  metadata?: Record<string, unknown>;
}

export interface A2ADataPart {
  kind: "data";
  data: Record<string, unknown>;
  metadata?: Record<string, unknown>;
}

export type A2APart = A2ATextPart | A2ADataPart;

export interface A2AMessage {
  kind: "message";
  id: string;
  role: "user" | "agent";
  parts: A2APart[];
  contextId?: string;
  taskId?: string;
  metadata?: Record<string, unknown>;
}

export type A2ATaskState =
  | "submitted"
  | "working"
  | "input-required"
  | "completed"
  | "canceled"
  | "failed"
  | "rejected"
  | "auth-required"
  | "unknown";

export interface A2AArtifact {
  artifactId: string;
  name?: string;
  description?: string;
  parts: A2APart[];
  metadata?: Record<string, unknown>;
}

export interface A2ATaskStatus {
  state: A2ATaskState;
  message?: A2AMessage;
  /** RFC 3339 timestamp. */
  timestamp: string;
}

export interface A2ATask {
  kind: "task";
  id: string;
  contextId?: string;
  status: A2ATaskStatus;
  history?: A2AMessage[];
  artifacts?: A2AArtifact[];
  metadata?: Record<string, unknown>;
}

/** JSON-RPC-shaped error envelope, which is what A2A transports return. */
export interface A2AErrorResponse {
  error: { code: number; message: string; data?: unknown };
}
