/**
 * The internal typed agent bus.
 *
 * This is the transport half of the "both" decision: every agent-to-agent call
 * in the platform goes through here, and the Google A2A wire format (see
 * `a2a.ts`) is layered on top as an envelope rather than as a parallel
 * implementation. One bus means one place where "is that agent loaded?" is
 * answered, so an unloaded plugin fails identically whether it is reached over
 * REST, over A2A, or from another agent.
 *
 * LOOP CAP
 * A broadcast agent that forwards to another broadcast agent, which forwards
 * back, is a real possibility once agents can call agents. Depth is tracked in
 * an `AsyncLocalStorage` rather than on the message, because the forwarding
 * agent constructs a *new* message and would not copy a `hops` field across.
 */

import { AsyncLocalStorage } from "node:async_hooks";
import { Context, Service } from "cordis";
import type { AgentMessage } from "./types.js";

export type AgentHandler = (message: AgentMessage) => unknown;

export interface DeliveryResult {
  /** Whatever the handlers returned, in delivery order. */
  replies: unknown[];
  /** One entry per handler that threw, so a caller can surface the reason. */
  errors: { agentId: string; error: unknown }[];
}

export interface AgentBusConfig {
  /**
   * Maximum number of handler crossings a single delivery chain may make.
   * Defaults to 8, which is comfortably above any real fan-out.
   */
  maxHops?: number;
  /** Default `request` timeout in ms. Defaults to 10s. */
  defaultTimeoutMs?: number;
}

/** Thrown when a message targets an agent that has no subscribers. */
export class AgentNotLoadedError extends Error {
  readonly agentId: string;
  constructor(agentId: string) {
    super(`agent "${agentId}" is not loaded`);
    this.name = "AgentNotLoadedError";
    this.agentId = agentId;
  }
}

export class AgentTimeoutError extends Error {
  readonly agentId: string;
  readonly timeoutMs: number;
  constructor(agentId: string, timeoutMs: number) {
    super(`agent "${agentId}" did not reply within ${timeoutMs}ms`);
    this.name = "AgentTimeoutError";
    this.agentId = agentId;
    this.timeoutMs = timeoutMs;
  }
}

export interface AgentBusEvents {
  /** Emitted whenever a skill handler reports intermediate progress. */
  "agent/progress"(progress: { from: string; skill: string; name: string; data: unknown }): void;
}

declare module "cordis" {
  interface Events extends AgentBusEvents {}
}

declare module "cordis" {
  interface Context {
    agentBus: AgentBus;
  }
}

interface HopState {
  depth: number;
}

export class AgentBus extends Service {
  static inject = [];

  private readonly _subscribers = new Map<string, Set<AgentHandler>>();
  private readonly _hops = new AsyncLocalStorage<HopState>();
  public config: AgentBusConfig;

  constructor(ctx: Context, config?: AgentBusConfig) {
    super(ctx, "agentBus");
    this.config = config ?? {};
  }

  get maxHops(): number {
    return this.config.maxHops ?? 8;
  }

  get defaultTimeoutMs(): number {
    return this.config.defaultTimeoutMs ?? 10_000;
  }

  /**
   * Subscribe an agent. Returns the unsubscribe function, which is what the
   * host holds onto: losing it on unload is how a re-enabled plugin avoids
   * answering every message twice.
   */
  subscribe(agentId: string, handler: AgentHandler): () => void {
    let handlers = this._subscribers.get(agentId);
    if (!handlers) {
      handlers = new Set();
      this._subscribers.set(agentId, handlers);
    }
    handlers.add(handler);
    let removed = false;
    return () => {
      if (removed) return;
      removed = true;
      this.unsubscribe(agentId, handler);
    };
  }

  unsubscribe(agentId: string, handler: AgentHandler): boolean {
    const handlers = this._subscribers.get(agentId);
    if (!handlers) return false;
    const removed = handlers.delete(handler);
    if (handlers.size === 0) this._subscribers.delete(agentId);
    return removed;
  }

  /** True when at least one agent answers under `agentId`. */
  has(agentId: string): boolean {
    const handlers = this._subscribers.get(agentId);
    return !!handlers && handlers.size > 0;
  }

  /** Agent ids that currently have subscribers. */
  agents(): string[] {
    return [...this._subscribers.keys()];
  }

  /**
   * Fan a message out to the subscribers of `to`, or to every subscriber when
   * `to` is undefined.
   *
   * Throws {@link AgentNotLoadedError} for an addressed-but-absent agent: a
   * message to a plugin that was unloaded must fail loudly rather than look
   * like a broadcast nobody heard.
   */
  async publish(message: AgentMessage): Promise<DeliveryResult> {
    return this._deliver(message);
  }

  /**
   * Send to one agent and await its reply.
   *
   * Rejects with {@link AgentNotLoadedError} if the agent is not loaded,
   * {@link AgentTimeoutError} on timeout, and with the handler's own error if
   * the only subscriber threw — a swallowed error here would surface to the
   * user as a bare "no reply", which is much harder to debug.
   */
  async request(message: AgentMessage, timeoutMs?: number): Promise<AgentMessage> {
    const limit = timeoutMs ?? this.defaultTimeoutMs;
    const agentId = message.to;
    if (agentId && !this.has(agentId)) throw new AgentNotLoadedError(agentId);

    let timer: ReturnType<typeof setTimeout> | undefined;
    const expiry = new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new AgentTimeoutError(agentId ?? "*", limit)), limit);
    });

    let result: DeliveryResult;
    try {
      result = await Promise.race([this._deliver(message), expiry]);
    } finally {
      if (timer) clearTimeout(timer);
    }

    const reply = result.replies.find((value) => value !== undefined);
    if (reply === undefined) {
      const failure = result.errors[0];
      if (failure) {
        const error = failure.error;
        throw error instanceof Error
          ? error
          : new Error(`agent "${agentId ?? "*"}" failed: ${String(error)}`);
      }
      throw new Error(`agent "${agentId ?? "*"}" produced no reply for skill "${message.skill}"`);
    }
    return this._asMessage(reply, message);
  }

  /**
   * Normalize whatever a handler returned into an {@link AgentMessage}.
   *
   * A value counts as an already-formed message when it carries `messageId`,
   * or when it carries `timestamp` alongside `text`/`data`. Requiring two of the
   * four keeps a skill that legitimately returns `{ text: "..." }` as plain
   * structured data, which is the more common mistake and the harder one to
   * spot in a handler.
   */
  private _asMessage(value: unknown, request: AgentMessage): AgentMessage {
    if (value && typeof value === "object" && !Array.isArray(value)) {
      const candidate = value as Record<string, unknown>;
      const hasText = "text" in candidate;
      const hasData = "data" in candidate;
      const looksLikeMessage =
        "messageId" in candidate ||
        ("timestamp" in candidate && (hasText || hasData)) ||
        (hasText && hasData);
      if (looksLikeMessage) return value as AgentMessage;
    }
    return {
      from: request.to ?? request.from,
      messageId: request.messageId,
      skill: request.skill,
      data: value,
      timestamp: Date.now(),
    };
  }

  private async _deliver(message: AgentMessage): Promise<DeliveryResult> {
    const depth = this._hops.getStore()?.depth ?? 0;
    if (depth >= this.maxHops) {
      this.ctx.logger.warn(
        "agentBus: dropping %s from %s, hop cap %s reached (broadcast loop?)",
        message.skill,
        message.from,
        this.maxHops,
      );
      return { replies: [], errors: [] };
    }

    let targets: string[];
    if (message.to) {
      if (!this.has(message.to)) throw new AgentNotLoadedError(message.to);
      targets = [message.to];
    } else {
      targets = this.agents();
    }

    const replies: unknown[] = [];
    const errors: { agentId: string; error: unknown }[] = [];

    await Promise.all(
      targets.map(async (agentId) => {
        const handlers = [...(this._subscribers.get(agentId) ?? [])];
        for (const handler of handlers) {
          try {
            const reply = await this._hops.run({ depth: depth + 1 }, () => handler(message));
            if (reply !== undefined) replies.push(reply);
          } catch (err) {
            // One agent's bug must not silence the others, so this is logged and
            // collected rather than rethrown. `request` re-raises it afterwards.
            this.ctx.logger.warn(
              "agentBus: agent %s threw handling %s: %s",
              agentId,
              message.skill,
              err instanceof Error ? err.message : String(err),
            );
            errors.push({ agentId, error: err });
          }
        }
      }),
    );

    return { replies, errors };
  }
}
