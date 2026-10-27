// The /cloud/v1 router (design 78 §25.10, D1741). Pure: every outside system
// comes in through AppDeps, so the same code runs under Workers and in tests.

import { b64url, EMAIL_IN_TEXT, hmacHex, ipPrefix, normaliseEmail, randomBytes, type Keys } from "./crypto.ts";
import { ApiError, clientIp, errorResponse, invalid, json, optString, rateLimited, readJson, reqString } from "./http.ts";
import type { Clock, Logger, RateLimiter, Turnstile, WaitUntil } from "./ports.ts";
import { SOURCES, type Source, type Waitlist } from "./waitlist.ts";

export const PREFIX = "/cloud/v1";

export interface AppDeps {
  keys: Keys;
  waitlist: Waitlist;
  turnstile: Turnstile;
  limiter: RateLimiter;
  clock: Clock;
  logger: Logger;
  /** Extra routes, added by later tasks (admin, invites). Return null when not matched. */
  routes?: ((ctx: RouteContext) => Promise<Response | null>)[];
}

export interface RouteContext {
  req: Request;
  url: URL;
  path: string; // without PREFIX
  requestId: string;
  ipPrefixHmac: string;
  ctx: WaitUntil;
}

/** §25.7's limits. */
export const LIMITS = {
  joinPer10Min: { limit: 5, windowMs: 10 * 60_000 },
  joinPerDay: { limit: 20, windowMs: 24 * 3600_000 },
  tokenPerMin: { limit: 30, windowMs: 60_000 },
};

async function limit(deps: AppDeps, key: string, l: { limit: number; windowMs: number }): Promise<void> {
  const r = await deps.limiter.hit(key, l.limit, l.windowMs);
  if (!r.ok) throw rateLimited(r.retryAfterMs / 1000);
}

function newRequestId(): string {
  return `req_${b64url(randomBytes(12))}`;
}

function optionalAnswers(body: Record<string, unknown>): { build?: string; source?: Source; sourceOther?: string } {
  const build = optString(body, "build", 500);
  const source = optString(body, "source", 16);
  const sourceOther = optString(body, "source_other", 100);
  if (source !== undefined && !(SOURCES as readonly string[]).includes(source)) throw invalid(`source must be one of ${SOURCES.join(", ")}`);
  if (sourceOther !== undefined && source !== "other") throw invalid("source_other goes with source \"other\"");
  // The optional answers are stored in plaintext, so they may not carry an address (§25.3).
  if ((build && EMAIL_IN_TEXT.test(build)) || (sourceOther && EMAIL_IN_TEXT.test(sourceOther))) {
    throw invalid("the optional answers can't contain an email address");
  }
  return { build, source: source as Source | undefined, sourceOther };
}

function emailField(body: Record<string, unknown>): string {
  const raw = reqString(body, "email", 254);
  const email = normaliseEmail(raw);
  if (!email) throw invalid("email is not a valid address");
  return email;
}

async function turnstileOk(deps: AppDeps, body: Record<string, unknown>, ip: string, requestId: string): Promise<void> {
  const token = reqString(body, "turnstile_token", 2048);
  if (!(await deps.turnstile.verify(token, ip, requestId))) throw new ApiError(403, "TURNSTILE_FAILED", "the challenge failed; reload the page and try again");
}

const CHECK = { status: "check_your_email" };

async function route(deps: AppDeps, rc: RouteContext): Promise<Response> {
  const { req, path, requestId, ipPrefixHmac, ctx } = rc;
  const ip = clientIp(req);

  if (path === "/waitlist" && req.method === "POST") {
    const body = await readJson(req);
    const email = emailField(body);
    const answers = optionalAnswers(body);
    await limit(deps, `join:10m:${ipPrefixHmac}`, LIMITS.joinPer10Min);
    await limit(deps, `join:1d:${ipPrefixHmac}`, LIMITS.joinPerDay);
    await turnstileOk(deps, body, ip, requestId);
    ctx.waitUntil(
      deps.waitlist.join({ email, ...answers, ipPrefixHmac }).catch((e) => deps.logger.log("waitlist.join_failed", { request_id: requestId, error: String(e) })),
    );
    return json(requestId, 202, CHECK);
  }

  if (path === "/waitlist/confirm" && req.method === "POST") {
    const body = await readJson(req);
    const token = reqString(body, "token", 128);
    await limit(deps, `token:1m:${ipPrefixHmac}`, LIMITS.tokenPerMin);
    if (!(await deps.waitlist.confirm(token))) throw new ApiError(410, "TOKEN_UNUSABLE", "this link has expired or was already used");
    return json(requestId, 200, { status: "confirmed" });
  }

  if (path === "/waitlist" && req.method === "DELETE") {
    const body = await readJson(req);
    if (typeof body.token === "string") {
      const token = reqString(body, "token", 128);
      await limit(deps, `token:1m:${ipPrefixHmac}`, LIMITS.tokenPerMin);
      if (!(await deps.waitlist.removeByToken(token))) throw new ApiError(410, "TOKEN_UNUSABLE", "this link has expired or was already used");
      return json(requestId, 204, null);
    }
    const email = emailField(body);
    await limit(deps, `join:10m:${ipPrefixHmac}`, LIMITS.joinPer10Min);
    await limit(deps, `join:1d:${ipPrefixHmac}`, LIMITS.joinPerDay);
    await turnstileOk(deps, body, ip, requestId);
    ctx.waitUntil(
      deps.waitlist.requestRemoval(email).catch((e) => deps.logger.log("waitlist.removal_failed", { request_id: requestId, error: String(e) })),
    );
    return json(requestId, 202, CHECK);
  }

  for (const r of deps.routes ?? []) {
    const res = await r(rc);
    if (res) return res;
  }
  throw new ApiError(404, "NOT_FOUND", "no such endpoint");
}

export async function handle(req: Request, deps: AppDeps, ctx: WaitUntil): Promise<Response> {
  const requestId = newRequestId();
  const url = new URL(req.url);
  const started = deps.clock.now();
  let res: Response;
  if (!url.pathname.startsWith(PREFIX + "/")) {
    res = errorResponse(requestId, new ApiError(404, "NOT_FOUND", "no such endpoint"));
  } else {
    const ipPrefixHmac = await hmacHex(deps.keys, "ip", ipPrefix(clientIp(req)));
    try {
      res = await route(deps, { req, url, path: url.pathname.slice(PREFIX.length), requestId, ipPrefixHmac, ctx });
    } catch (e) {
      if (e instanceof ApiError) res = errorResponse(requestId, e);
      else {
        deps.logger.log("request.error", { request_id: requestId, error: String(e) });
        res = errorResponse(requestId, new ApiError(500, "INTERNAL", "something went wrong; try again"));
      }
    }
  }
  // The path only: no query string (it may carry codes) and never a fragment.
  deps.logger.log("request", { request_id: requestId, method: req.method, path: url.pathname, status: res.status, ms: deps.clock.now() - started });
  return res;
}
