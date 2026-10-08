/**
 * Who is making a request, and what they may do.
 *
 * There are exactly three kinds of principal, and they are mutually exclusive on
 * purpose: a machine acting for an agent is not a logged-in human, and treating it as
 * one would let a service token inherit the human's session scopes.
 *
 *   user          - a browser session established by the OIDC code flow.
 *   service-token - a bearer token minted for one agent, scoped to its declared skills.
 *   anonymous     - nobody. Produced when auth is disabled or no credential was sent.
 *
 * `ANONYMOUS` is NOT an admin principal. That distinction is the whole of the
 * "never silently enable admin powers without auth configured" rule: with no IdP
 * configured the app still boots and still serves the dashboard, but every privileged
 * operation is refused.
 */

import type { AuthUser } from "./session.js";

export interface UserPrincipal {
  kind: "user";
  /** sha256 of the session cookie. Never the cookie value itself. */
  sessionIdHash: string;
  user: AuthUser;
  /** Scopes the IdP actually granted at login. */
  scopes: string[];
}

export interface ServiceTokenPrincipal {
  kind: "service-token";
  tokenId: string;
  name: string;
  /** The agent this token may act as, if it was minted for one. */
  agentId?: string;
  /** Skill ids this token may invoke. */
  skills: string[];
  scopes: string[];
}

export interface AnonymousPrincipal {
  kind: "anonymous";
  scopes: [];
  /** Why the principal is anonymous, when auth is disabled. */
  reason?: string;
}

export type AuthPrincipal = UserPrincipal | ServiceTokenPrincipal | AnonymousPrincipal;

export const ANONYMOUS: AnonymousPrincipal = { kind: "anonymous", scopes: [] };

export function principalScopes(principal: AuthPrincipal | undefined): string[] {
  return principal?.scopes ?? [];
}

/** The signed-in user, or undefined for a service token and for anonymous. */
export function principalUser(principal: AuthPrincipal | undefined): AuthUser | undefined {
  return principal?.kind === "user" ? principal.user : undefined;
}

export type ScopeVerdict = { ok: true; missing: [] } | { ok: false; missing: string[] };

/**
 * Is every required scope held by this principal?
 *
 * THE ONE RULE THAT MATTERS: this is evaluated against the principal of the CURRENT
 * request, never against a flag captured when the plugin was enabled. A session created
 * before a plugin was deployed predates that plugin's existence and cannot possibly have
 * been granted its scopes, so an enable-time check would wave it straight through. This
 * function is therefore called from the per-request guard, not from `enable`.
 */
export function evaluateScopeGate(
  required: string[] | undefined,
  principal: AuthPrincipal | undefined,
): ScopeVerdict {
  if (!required || required.length === 0) return { ok: true, missing: [] };
  const held = new Set(principalScopes(principal));
  const missing = required.filter((scope) => !held.has(scope));
  return missing.length === 0 ? { ok: true, missing: [] } : { ok: false, missing };
}

export function principalHasScope(principal: AuthPrincipal | undefined, scope: string): boolean {
  return principalScopes(principal).includes(scope);
}

/** Admin is a SCOPE, not a flag. Returns false for anonymous by construction. */
export function principalIsAdmin(
  principal: AuthPrincipal | undefined,
  adminScope: string | undefined,
): boolean {
  if (!adminScope) return false;
  return principalScopes(principal).includes(adminScope);
}

/**
 * Which skills may this principal invoke on `agentId`?
 *
 * A service token is scoped to one agent AND to a list of that agent's declared skills.
 * A token minted for agent A therefore cannot drive agent B even if it is otherwise
 * valid, and cannot reach a skill A never declared. A user session may invoke any skill
 * the agent declares.
 */
export function principalCanInvokeSkill(
  principal: AuthPrincipal | undefined,
  agentId: string,
  skillId: string,
): { ok: true } | { ok: false; reason: string } {
  if (!principal || principal.kind === "anonymous") {
    return { ok: false, reason: "anonymous" };
  }
  if (principal.kind === "user") return { ok: true };
  if (principal.agentId && principal.agentId !== agentId) {
    return { ok: false, reason: "wrong-agent" };
  }
  if (principal.skills.length > 0 && !principal.skills.includes(skillId)) {
    return { ok: false, reason: "skill-not-granted" };
  }
  return { ok: true };
}
