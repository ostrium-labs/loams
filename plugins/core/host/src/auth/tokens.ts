/**
 * Per-agent service tokens for machine callers.
 *
 * These exist because `message:send` has two kinds of legitimate caller with genuinely
 * different powers: a signed-in human, and an agent process acting on its own. A human
 * brings a session; an agent brings a bearer token minted for it.
 *
 * The token is random, shown exactly once, and stored HASHED. It is scoped twice over:
 * to one `agentId`, and to a subset of that agent's declared skills. Both bounds matter.
 * Scoping only to the agent would let a token invoke any skill the agent declares,
 * including one added later by a new plugin version; scoping only to skills would let a
 * token minted for one agent drive another.
 *
 * The plaintext is never recoverable after creation, which is why `createServiceToken`
 * returns it separately from the stored record.
 */

import { randomBytes, randomUUID } from "node:crypto";
import type { ServiceTokenRecord } from "./session.js";
import { sha256 } from "./session.js";

export interface CreateServiceTokenInput {
  name: string;
  /** Restrict the token to one agent. Omit for a token valid across agents. */
  agentId?: string;
  /** Skill ids the token may invoke. Empty means "any skill the agent declares". */
  skills?: string[];
  scopes?: string[];
  /** Absolute lifetime. Omit for a non-expiring token. */
  ttlMs?: number;
}

export interface CreatedServiceToken {
  record: ServiceTokenRecord;
  /** Returned ONCE. Never stored, never recoverable afterwards. */
  token: string;
}

export function createServiceToken(input: CreateServiceTokenInput): CreatedServiceToken {
  const secret = randomBytes(32).toString("base64url");
  const token = `bis_${secret}`;
  const createdAt = Date.now();
  const record: ServiceTokenRecord = {
    id: randomUUID(),
    tokenHash: sha256(token),
    name: input.name,
    agentId: input.agentId,
    skills: [...(input.skills ?? [])],
    scopes: [...(input.scopes ?? [])],
    createdAt,
    expiresAt: input.ttlMs && input.ttlMs > 0 ? createdAt + input.ttlMs : undefined,
  };
  return { record, token };
}

/** Parse a service token off an `Authorization: Bearer ...` header. */
export function readBearerToken(header: string | undefined): string | undefined {
  if (!header) return undefined;
  const match = /^Bearer\s+(.+)$/i.exec(header.trim());
  return match?.[1]?.trim();
}

/**
 * Is this stored token still usable at `now`?
 *
 * Revocation is checked before expiry so a revoked token reports as revoked in logs,
 * which is the difference between "this was cancelled" and "this expired last Tuesday"
 * during an incident.
 */
export function serviceTokenUsable(
  record: ServiceTokenRecord | undefined,
  now: number,
): { usable: true } | { usable: false; reason: "missing" | "revoked" | "expired" } {
  if (!record) return { usable: false, reason: "missing" };
  if (record.revokedAt !== undefined && record.revokedAt !== null) {
    return { usable: false, reason: "revoked" };
  }
  if (record.expiresAt !== undefined && record.expiresAt <= now) {
    return { usable: false, reason: "expired" };
  }
  return { usable: true };
}
