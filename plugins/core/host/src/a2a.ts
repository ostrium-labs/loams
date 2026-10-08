/**
 * Google A2A, as an envelope over the internal bus.
 *
 * The platform has one transport (`AgentBus`) and one external contract (A2A).
 * The temptation is to implement A2A as a second, parallel dispatch path, which
 * would immediately produce two answers to "is this agent loaded?" and two
 * places to get A2A error shapes wrong. Instead every `message:send` is
 * translated into an `AgentMessage`, handed to the bus, and translated back:
 * this file is protocol translation, `bus.ts` is delivery.
 *
 * FIDELITY NOTES
 * The shapes below follow the A2A JSON-RPC/HTTP binding with camelCase members:
 * `AgentCard` (capabilities, defaultInputModes/OutputModes, skills), `Message`
 * (kind: "message", role, parts of kind "text" / "data"), and `Task` (kind:
 * "task", status.state, history, artifacts). An `Artifact` carries structured
 * results as a `DataPart`, which is how `message:send` returns anything other
 * than prose.
 *
 * The transport here is `POST /a2a/v1/message:send` rather than a JSON-RPC
 * envelope, so the response body is the bare `Task` (or `Message`) object and
 * failures use A2A's `{ error: { code, message, data } }` envelope alongside a
 * meaningful HTTP status.
 *
 * KNOWN DEVIATION (recorded, not accidental)
 * A2A v1.0 names its method `message/send` and carries it in a JSON-RPC 2.0
 * envelope. This server uses the repo's other convention instead -- the
 * ConnectRPC-style `/a2a/v1/<method>:<verb>` path with the bare `Task` as the
 * response -- and the two are not the same wire protocol. The reason is that
 * `message:send` is the ONLY method implemented: this file translates one call
 * onto the bus, and adding a second dispatch path for a partially implemented
 * JSON-RPC envelope would give the platform two answers to "which agents
 * exist". The card path was corrected to the v1.0 spelling (`agent-card.json`,
 * see {@link AGENT_CARD_PATH}) because that one is a discovery URL a client
 * fetches by convention and there is nothing to gain from diverging there.
 * Migrating `message:send` to a full JSON-RPC envelope is a separate change and
 * would break every existing caller.
 */

import { randomUUID } from "node:crypto";
import type { Context } from "cordis";
import type { HttpRouter, RouteRequest } from "./router.js";
import type { AgentBus } from "./bus.js";
import type { PluginRegistry } from "./registry.js";
import type { RequestLike } from "./auth/service.js";
import { evaluateScopeGate, principalCanInvokeSkill, type AuthPrincipal } from "./auth/scopes.js";
import {
  A2A_PROTOCOL_VERSION,
  type A2AAgentCard,
  type A2AAgentSkill,
  type A2ADataPart,
  type A2AErrorResponse,
  type A2AMessage,
  type A2APart,
  type A2ATask,
  type A2ATaskState,
  type A2ATextPart,
  type AgentMessage,
  type PluginAgentSkill,
} from "./types.js";

/* -------------------------------------------------------------------------- */
/* Errors                                                                     */
/* -------------------------------------------------------------------------- */

/** JSON-RPC error codes used by this server. */
export const A2AErrorCode = {
  /** The request body was not a well-formed A2A message. */
  InvalidRequest: -32602,
  /** A named skill or method does not exist. */
  NotFound: -32601,
  /** The agent exists but its plugin is not loaded. */
  AgentNotLoaded: -32001,
  /** The skill itself failed. */
  SkillFailed: -32002,
  /** No usable credential was presented. */
  Unauthenticated: -32003,
  /** Authenticated, but not permitted to invoke this agent or skill. */
  PermissionDenied: -32004,
  /** Internal failure. */
  Internal: -32603,
} as const;

export class A2AError extends Error {
  readonly code: number;
  readonly httpStatus: number;
  readonly data?: unknown;

  constructor(code: number, message: string, httpStatus: number, data?: unknown) {
    super(message);
    this.name = "A2AError";
    this.code = code;
    this.httpStatus = httpStatus;
    if (data !== undefined) this.data = data;
  }

  toResponse(): A2AErrorResponse {
    const error: A2AErrorResponse["error"] = { code: this.code, message: this.message };
    if (this.data !== undefined) error.data = this.data;
    return { error };
  }
}

/* -------------------------------------------------------------------------- */
/* Agent cards                                                                */
/* -------------------------------------------------------------------------- */

export interface AgentCardOptions {
  /** Base URL clients should call back on, e.g. `http://localhost:3001`. */
  baseUrl: string;
  /** Extra skills merged into the aggregate card. */
  extraSkills?: PluginAgentSkill[];
}

/**
 * Where the platform's aggregate Agent Card is served.
 *
 * A2A v1.0 specifies `/.well-known/agent-card.json`. The pre-1.0 spelling
 * `/.well-known/agent.json` is still served as an alias at {@link
 * LEGACY_AGENT_CARD_PATH} -- it is one route, one handler and one body, so the
 * alias cannot drift from the canonical path, and it exists only so a client
 * written against the old path keeps working.
 */
export const AGENT_CARD_PATH = "/.well-known/agent-card.json";

/** Pre-v1.0 spelling of {@link AGENT_CARD_PATH}. Kept working, never advertised. */
export const LEGACY_AGENT_CARD_PATH = "/.well-known/agent.json";

export function buildSkillCard(skill: PluginAgentSkill): A2AAgentSkill {
  const card: A2AAgentSkill = {
    id: skill.id,
    name: skill.name,
    description: skill.description,
  };
  if (skill.tags) card.tags = skill.tags;
  if (skill.examples) card.examples = skill.examples;
  if (skill.inputModes) card.inputModes = skill.inputModes;
  if (skill.outputModes) card.outputModes = skill.outputModes;
  return card;
}

/** One agent's card, derived from the plugin manifest that owns it. */
export function buildAgentCard(
  manifest: {
    id: string;
    name: string;
    description: string;
    agent?: { name: string; description: string; version: string; skills: PluginAgentSkill[] };
  },
  options: AgentCardOptions,
): A2AAgentCard {
  const agent = manifest.agent;
  return {
    protocolVersion: A2A_PROTOCOL_VERSION,
    name: agent?.name ?? manifest.name,
    description: agent?.description ?? manifest.description,
    version: agent?.version ?? "0.0.0",
    url: `${options.baseUrl}/a2a/v1/message:send`,
    capabilities: {
      // The bus is in-process and synchronous-per-hop; there is no SSE stream
      // to attach to, and claiming one would be a lie a client would rely on.
      streaming: false,
      pushNotifications: false,
      stateTransitionHistory: true,
    },
    defaultInputModes: ["application/json"],
    defaultOutputModes: ["application/json", "text/plain"],
    skills: (agent?.skills ?? []).map(buildSkillCard),
    additionalInterfaces: [
      { url: `${options.baseUrl}/.well-known/agent-card/${manifest.id}`, transport: "JSON" },
    ],
  };
}

/**
 * The platform's aggregate card.
 *
 * Skill ids are namespaced as `<agentId>.<skillId>` here because two adapters
 * are perfectly entitled to both declare `listChannels`, and an aggregate card
 * that silently collapsed them would route a client's call to whichever loaded
 * last. Per-agent cards use the bare skill id.
 */
export function buildAggregateCard(
  plugins: {
    id: string;
    name: string;
    description: string;
    agent?: { skills: PluginAgentSkill[] };
  }[],
  options: AgentCardOptions,
): A2AAgentCard {
  const skills: A2AAgentSkill[] = [];
  for (const plugin of plugins) {
    if (!plugin.agent) continue;
    for (const skill of plugin.agent.skills) {
      skills.push({
        ...buildSkillCard(skill),
        id: `${plugin.id}.${skill.id}`,
        description: `${plugin.name}: ${skill.description}`,
        tags: [...(skill.tags ?? []), plugin.id],
      });
    }
  }
  for (const extra of options.extraSkills ?? []) {
    skills.push(buildSkillCard(extra));
  }

  return {
    protocolVersion: A2A_PROTOCOL_VERSION,
    name: "Loams Plugin Host",
    description:
      "Cordis plugin host for Loams. Agents are contributed by loaded plugins; " +
      "everything here disappears when its plugin is unloaded.",
    version: "1.0.0",
    url: `${options.baseUrl}/a2a/v1/message:send`,
    capabilities: { streaming: false, pushNotifications: false, stateTransitionHistory: true },
    defaultInputModes: ["application/json"],
    defaultOutputModes: ["application/json", "text/plain"],
    skills,
    additionalInterfaces: plugins
      .filter((plugin) => plugin.agent)
      .map((plugin) => ({
        url: `${options.baseUrl}/.well-known/agent-card/${plugin.id}`,
        transport: "JSON",
      })),
  };
}

/* -------------------------------------------------------------------------- */
/* Tasks                                                                      */
/* -------------------------------------------------------------------------- */

function nowIso(): string {
  return new Date().toISOString();
}

/**
 * Accumulates a `Task` across several steps.
 *
 * A single-shot `message:send` only needs one artifact, but a multi-step skill
 * (fetch a dashboard, then describe a widget, then compile it) produces several
 * and the client needs them in order plus the conversation that led there.
 * Building this by hand as an object literal is how the `history` array and the
 * `artifacts` array drift apart.
 */
export class A2ATaskBuilder {
  readonly id: string;
  readonly contextId?: string;
  private _history: A2AMessage[] = [];
  private _artifacts: A2AArtifactLike[] = [];
  private _state: A2ATaskState = "working";
  private _statusMessage?: A2AMessage;

  constructor(id?: string, contextId?: string) {
    this.id = id ?? `task-${randomUUID()}`;
    if (contextId) this.contextId = contextId;
  }

  get state(): A2ATaskState {
    return this._state;
  }

  /** Record an inbound or outbound message in the conversation. */
  record(message: A2AMessage): this {
    this._history.push(message);
    return this;
  }

  /** Add a structured or textual result. */
  addArtifact(name: string, parts: A2APart[], description?: string): this {
    const artifact: A2AArtifactLike = { artifactId: randomUUID(), name, parts };
    if (description) artifact.description = description;
    this._artifacts.push(artifact);
    return this;
  }

  /** Convenience for the common case: a JSON result artifact. */
  addDataArtifact(name: string, data: unknown, description?: string): this {
    const parts: A2APart[] = [{ kind: "data", data: data as Record<string, unknown> }];
    const text = typeof data === "string" ? data : undefined;
    if (text) parts.unshift({ kind: "text", text });
    return this.addArtifact(name, parts, description);
  }

  complete(message?: A2AMessage): this {
    this._state = "completed";
    if (message) this._statusMessage = message;
    return this;
  }

  fail(message: A2AMessage): this {
    this._state = "failed";
    this._statusMessage = message;
    return this;
  }

  toJSON(): A2ATask {
    const status: A2ATask["status"] = { state: this._state, timestamp: nowIso() };
    if (this._statusMessage) status.message = this._statusMessage;
    const task: A2ATask = { kind: "task", id: this.id, status };
    if (this.contextId) task.contextId = this.contextId;
    if (this._history.length > 0) task.history = this._history;
    if (this._artifacts.length > 0) task.artifacts = this._artifacts;
    return task;
  }
}

interface A2AArtifactLike {
  artifactId: string;
  name?: string;
  description?: string;
  parts: A2APart[];
}

/** Build an A2A `Message` from an internal bus message. */
export function buildAgentMessage(
  reply: AgentMessage,
  role: "user" | "agent" = "agent",
): A2AMessage {
  const parts: A2APart[] = [];
  if (reply.text) parts.push({ kind: "text", text: reply.text });
  if (reply.data !== undefined) {
    parts.push({
      kind: "data",
      data:
        reply.data && typeof reply.data === "object"
          ? (reply.data as Record<string, unknown>)
          : { value: reply.data },
    });
  }
  if (parts.length === 0) parts.push({ kind: "text", text: "" });
  return { kind: "message", id: reply.messageId, role, parts };
}

/* -------------------------------------------------------------------------- */
/* Request parsing                                                            */
/* -------------------------------------------------------------------------- */

export interface ParsedSendRequest {
  agentId: string;
  skill: string;
  params: Record<string, unknown>;
  message: A2AMessage;
  messageId: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

/**
 * Pull `agent`, `skill` and `params` out of an A2A message.
 *
 * The skill travels in a `DataPart` because it is structured routing
 * information, not conversation: the `TextPart` is whatever the human or the
 * LLM said, and parsing routing out of prose is the thing A2A explicitly
 * avoids. A shorthand `{ agent, skill, params }` body is also accepted so the
 * endpoint is usable from `curl` without building a message envelope.
 */
export function parseSendRequest(body: unknown): ParsedSendRequest {
  if (!isRecord(body)) {
    throw new A2AError(
      A2AErrorCode.InvalidRequest,
      "Request body must be a JSON object containing an A2A `message`.",
      400,
    );
  }

  const raw = body.message ?? body;
  if (!isRecord(raw)) {
    throw new A2AError(A2AErrorCode.InvalidRequest, "`message` must be an object.", 400);
  }
  const parts = (raw as unknown as A2AMessage).parts;
  if (!Array.isArray(parts) || parts.length === 0) {
    throw new A2AError(
      A2AErrorCode.InvalidRequest,
      "`message.parts` must be a non-empty array of A2A parts.",
      400,
    );
  }
  for (const part of parts) {
    if (!isRecord(part) || (part.kind !== "text" && part.kind !== "data")) {
      throw new A2AError(
        A2AErrorCode.InvalidRequest,
        'Each part must be an A2A part with kind "text" or "data".',
        400,
      );
    }
    if (part.kind === "data" && !isRecord(part.data)) {
      throw new A2AError(A2AErrorCode.InvalidRequest, "A data part must carry an object.", 400);
    }
  }

  const metadata = isRecord(raw.metadata) ? raw.metadata : undefined;
  const dataPart = parts.find((part) => isRecord(part) && part.kind === "data") as
    | A2ADataPart
    | undefined;

  const agentId = (dataPart?.data?.agent ?? dataPart?.data?.agentId ?? metadata?.agent) as
    | string
    | undefined;
  const skill = (dataPart?.data?.skill ?? metadata?.skill) as string | undefined;
  const params = (dataPart?.data?.params ?? metadata?.params ?? {}) as Record<string, unknown>;

  if (typeof agentId !== "string" || agentId.length === 0) {
    throw new A2AError(
      A2AErrorCode.InvalidRequest,
      'No target agent: put `agent` in a data part\'s `data` (e.g. {kind:"data",data:{agent:"dashboard",skill:"listDashboards"}}).',
      400,
    );
  }
  if (typeof skill !== "string" || skill.length === 0) {
    throw new A2AError(
      A2AErrorCode.InvalidRequest,
      "No skill to invoke: put `skill` in a data part's `data`.",
      400,
    );
  }
  if (!isRecord(params)) {
    throw new A2AError(A2AErrorCode.InvalidRequest, "`params` must be an object.", 400);
  }

  return {
    agentId,
    skill,
    params,
    message: raw as unknown as A2AMessage,
    messageId: (raw.id as string) ?? `msg-${randomUUID()}`,
  };
}

/* -------------------------------------------------------------------------- */
/* Dispatch                                                                   */
/* -------------------------------------------------------------------------- */

export interface A2ADeps {
  registry: PluginRegistry;
  bus: AgentBus;
  ctx: Context;
  /** Base URL used to build card links. */
  baseUrl: () => string;
  /** Authenticates the caller. Absent => `message:send` is unauthenticated. */
  auth?: AuthGateway;
}

/**
 * The slice of `ctx.auth` the A2A layer needs.
 *
 * Declared structurally rather than importing `AuthService` so this layer does not depend
 * on the concrete auth implementation, and so it can be satisfied by a stub in tests.
 */
export interface AuthGateway {
  principal(request: RequestLike): Promise<AuthPrincipal>;
  /** The admin scope, or undefined while auth is disabled. */
  readonly adminScope?: string;
}

/**
 * Send one A2A message and return a terminal `Task`.
 *
 * Refuses an agent whose plugin is not loaded with the same "not loaded" error
 * the bus would produce, so A2A and REST agree about what exists.
 */
export async function sendMessage(
  deps: A2ADeps,
  body: unknown,
  principal?: AuthPrincipal,
): Promise<A2ATask> {
  const request = parseSendRequest(body);
  const { registry, bus } = deps;

  const status = registry.find(request.agentId);
  if (!status) {
    throw new A2AError(A2AErrorCode.NotFound, `Unknown agent: ${request.agentId}`, 404, {
      agents: bus.agents(),
    });
  }
  if (!status.enabled) {
    throw new A2AError(
      A2AErrorCode.AgentNotLoaded,
      `Agent "${request.agentId}" is not loaded: its plugin is disabled.`,
      404,
      { agent: request.agentId, pluginState: status.state },
    );
  }
  if (status.state === "unloaded") {
    throw new A2AError(
      A2AErrorCode.AgentNotLoaded,
      `Agent "${request.agentId}" is not loaded.`,
      404,
      { agent: request.agentId, pluginState: status.state },
    );
  }

  // Who is calling? This endpoint was the network-reachable one, so it is the one that
  // must not be anonymous.
  //
  // When auth is configured, refuse anonymous BEFORE anything else, so an unauthenticated
  // caller cannot use the error text to enumerate which agents exist.
  if (deps.auth) {
    if (!principal || principal.kind === "anonymous") {
      throw new A2AError(
        A2AErrorCode.Unauthenticated,
        "message:send requires either a signed-in user session or an agent service token.",
        401,
      );
    }
    // A service token is scoped to ONE agent and to a subset of its declared skills. Both
    // bounds are checked, so a token minted for agent A cannot drive agent B and cannot
    // reach a skill A never declared (including one a later plugin version adds).
    const tokenAgent = principal.kind === "service-token" ? principal.agentId : undefined;
    const permitted = principalCanInvokeSkill(principal, request.agentId, request.skill);
    if (!permitted.ok) {
      throw new A2AError(
        A2AErrorCode.PermissionDenied,
        permitted.reason === "wrong-agent"
          ? `This service token is scoped to agent "${tokenAgent}" and may not ` +
              `drive "${request.agentId}".`
          : `This service token is not granted skill "${request.skill}".`,
        403,
        { agent: request.agentId, skill: request.skill, reason: permitted.reason },
      );
    }
    // The plugin's own declared `requiredScopes`, checked against the CALLER. This is the
    // request-time half of enforcement: a session minted before the plugin was deployed
    // does not hold its scopes and must not be able to drive it.
    const verdict = evaluateScopeGate(status.requiredScopes, principal);
    if (!verdict.ok) {
      throw new A2AError(
        A2AErrorCode.PermissionDenied,
        `Agent "${request.agentId}" requires scope${verdict.missing.length === 1 ? "" : "s"} ` +
          `${verdict.missing.join(", ")}, which this caller does not hold.`,
        403,
        { agent: request.agentId, missingScopes: verdict.missing },
      );
    }
  }

  const declared = status.agent?.skills ?? [];
  if (declared.length > 0 && !declared.some((skill) => skill.id === request.skill)) {
    throw new A2AError(
      A2AErrorCode.NotFound,
      `Agent "${request.agentId}" does not declare skill "${request.skill}".`,
      404,
      { validSkills: declared.map((skill) => skill.id) },
    );
  }

  const text = request.message.parts.find(
    (part): part is A2ATextPart => isRecord(part) && part.kind === "text",
  )?.text;

  const outbound: AgentMessage = {
    from: "a2a-client",
    to: request.agentId,
    messageId: request.messageId,
    skill: request.skill,
    params: request.params,
    timestamp: Date.now(),
  };
  if (text !== undefined) outbound.text = text;

  const builder = new A2ATaskBuilder(`task-${request.messageId}`, request.message.contextId);
  builder.record(request.message);

  let reply: AgentMessage;
  try {
    reply = await bus.request(outbound, bus.defaultTimeoutMs);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const failure = buildAgentMessage(
      {
        from: request.agentId,
        messageId: `msg-${randomUUID()}`,
        skill: request.skill,
        text: message,
        timestamp: Date.now(),
      },
      "agent",
    );
    builder.record(failure).fail(failure);
    return builder.toJSON();
  }

  const replyMessage = buildAgentMessage(reply, "agent");
  builder.record(replyMessage);
  builder.addDataArtifact(
    `${request.skill}-result`,
    reply.data === undefined ? { text: reply.text ?? "" } : reply.data,
    `Result of ${request.skill}`,
  );
  return builder.complete(replyMessage).toJSON();
}

/* -------------------------------------------------------------------------- */
/* Routes                                                                     */
/* -------------------------------------------------------------------------- */

/**
 * Mount the A2A surface into a router.
 *
 * Routes are ordinary router entries, so they participate in the same
 * add/remove lifecycle as everything else — which is why a disabled plugin's
 * agent stops being addressable rather than answering from a stale closure.
 */
export function mountA2A(deps: A2ADeps, router: HttpRouter): () => void {
  const fail = (sendJson: (status: number, data: unknown) => void, err: unknown) => {
    const error =
      err instanceof A2AError
        ? err
        : new A2AError(
            A2AErrorCode.Internal,
            err instanceof Error ? err.message : String(err),
            500,
          );
    sendJson(error.httpStatus, error.toResponse());
  };

  const aggregateCardHandler = async ({ sendJson }: RouteRequest): Promise<boolean> => {
    // LOADED only, not merely "declares an agent". `sendMessage` refuses an
    // agent whose plugin is off, so advertising one here would make the
    // aggregate card promise a route that answers 404 -- and would be the
    // exact REST/A2A disagreement this file is layered on the bus to avoid.
    const loaded = deps.registry
      .list()
      .filter((plugin) => plugin.agent && plugin.enabled && plugin.state === "loaded");
    sendJson(200, buildAggregateCard(loaded, { baseUrl: deps.baseUrl() }));
    return true;
  };

  return router.addAll(
    [
      {
        // SIGNING: the cards served here are UNSIGNED. The loams-dev design calls
        // for a JWS (RFC 7515) over an RFC 8785 canonicalised payload, which is
        // not something to improvise: RFC 8785 has exact rules for member
        // ordering and for ECMAScript number serialisation, and a signature over
        // a payload that is merely `JSON.stringify`-canonical is not the scheme
        // the design specifies. It is a known follow-up, not an oversight, and
        // the card shape deliberately leaves room for it (a JWS travels as
        // `application/jose`, alongside this document rather than inside it).
        name: "a2a:agent-card-json",
        method: "GET",
        match: AGENT_CARD_PATH,
        handler: aggregateCardHandler,
      },
      {
        // Legacy alias. Registered as its own entry rather than as a redirect:
        // a 301 would break any client that does not follow redirects, and
        // serving the identical body from one handler cannot drift.
        name: "a2a:agent-json",
        method: "GET",
        match: LEGACY_AGENT_CARD_PATH,
        handler: aggregateCardHandler,
      },
      {
        name: "a2a:agent-card",
        method: "GET",
        match: /^\/\.well-known\/agent-card\/([a-zA-Z0-9._-]+)$/,
        handler: async ({ sendJson, path }) => {
          const id = decodeURIComponent(path.split("/").pop() ?? "");
          const status = deps.registry.find(id);
          if (!status) {
            fail(sendJson, new A2AError(A2AErrorCode.NotFound, `Unknown agent: ${id}`, 404));
            return true;
          }
          if (!status.agent) {
            fail(
              sendJson,
              new A2AError(A2AErrorCode.NotFound, `Plugin "${id}" exposes no agent.`, 404),
            );
            return true;
          }
          if (!status.enabled) {
            fail(
              sendJson,
              new A2AError(
                A2AErrorCode.AgentNotLoaded,
                `Agent "${id}" is not loaded: its plugin is disabled.`,
                404,
                { agent: id, pluginState: status.state },
              ),
            );
            return true;
          }
          sendJson(200, buildAgentCard(status, { baseUrl: deps.baseUrl() }));
          return true;
        },
      },
      {
        name: "a2a:message-send",
        method: "POST",
        match: "/a2a/v1/message:send",
        handler: async ({ req, sendJson, readJson }) => {
          try {
            const principal = deps.auth ? await deps.auth.principal({ req }) : undefined;
            const task = await sendMessage(deps, await readJson(), principal);
            sendJson(200, task);
          } catch (err) {
            fail(sendJson, err);
          }
          return true;
        },
      },
    ],
    "core:a2a",
  );
}

/** Convenience wrapper for `mountA2A` that reads its deps off the context. */
export function mountA2AOnContext(ctx: Context, baseUrl: string): () => void {
  return mountA2A(
    {
      registry: ctx.coreRegistry,
      bus: ctx.agentBus,
      ctx,
      baseUrl: () => baseUrl,
    },
    ctx.router,
  );
}
