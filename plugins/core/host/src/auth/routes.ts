/**
 * The `/api/auth/*` surface.
 *
 *   GET  /api/auth/login              -> 302 to the authorization endpoint
 *   GET  /api/auth/callback           -> exchange the code, create a session, 302 onward
 *   GET  /api/auth/logout             -> end the session, 302 to the provider
 *   GET  /api/auth/me                 -> current user + scopes (never any token)
 *   POST /api/auth/backchannel-logout -> validate a logout_token, destroy sessions
 *
 * NOTHING here returns a token to the browser. Access tokens are JWTs carrying groups,
 * email, picture and azp, they are always sizeable, and handing one to a single-page app
 * would put it in reach of any XSS on the page. The SPA receives a cookie and asks
 * `/api/auth/me` what it is allowed to do. There is also no UserInfo call anywhere: with
 * `include_claims_in_id_token` on (the default) the grant response's claims already carry
 * everything, so a second round trip would add a failure mode for nothing.
 */

import type { Context } from "cordis";
import type { HttpRouter } from "../router.js";
import { AuthError } from "./errors.js";
import type { AuthService } from "./service.js";

export interface AuthRoutesOptions {
  /** Where the SPA lives, used as the login `returnTo` default. */
  defaultReturnTo?: string;
}

function readFormBody(body: string): URLSearchParams {
  return new URLSearchParams(body);
}

/**
 * Register the auth routes.
 *
 * The back-channel logout body is `application/x-www-form-urlencoded`, which the router's
 * `readJson` cannot parse, so this route reads the raw request itself.
 */
export function mountAuthApi(ctx: Context, options: AuthRoutesOptions = {}): () => void {
  const auth: AuthService = ctx.auth;
  const router: HttpRouter = ctx.router;

  return router.addAll(
    [
      {
        name: "auth:login",
        method: "GET",
        match: "/api/auth/login",
        handler: async ({ res, url, sendJson }) => {
          try {
            // `return_to` is only honoured when it is a LOCAL path. Accepting an absolute
            // URL here would make this an open redirect: an attacker sends a victim to
            // /api/auth/login?return_to=https://evil.example and the freshly minted
            // session's authorization code lands on a host the victim trusts us with.
            const target = sanitizeReturnTo(url.searchParams.get("return_to"));
            const destination = await auth.beginLogin(target ?? options.defaultReturnTo);
            res.writeHead(302, { Location: destination, "Cache-Control": "no-store" });
            res.end();
          } catch (err) {
            sendJson(statusOf(err), { error: messageOf(err), code: codeOf(err) });
          }
          return true;
        },
      },
      {
        name: "auth:callback",
        method: "GET",
        match: "/api/auth/callback",
        handler: async ({ res, url, sendJson }) => {
          try {
            const result = await auth.completeLogin(url);
            res.writeHead(302, {
              Location: result.redirectTo,
              "Set-Cookie": result.setCookie,
              "Cache-Control": "no-store",
            });
            res.end();
          } catch (err) {
            // An actionable configuration failure must arrive as a legible error naming the
            // Authentik setting to change, never as a bare 500.
            sendJson(statusOf(err), { error: messageOf(err), code: codeOf(err) });
          }
          return true;
        },
      },
      {
        name: "auth:logout",
        method: "GET",
        match: "/api/auth/logout",
        handler: async ({ req, res, sendJson }) => {
          try {
            const result = await auth.logout({ req });
            const headers: Record<string, string> = {
              "Set-Cookie": result.setCookie,
              "Cache-Control": "no-store",
            };
            if (result.redirectTo) headers.Location = result.redirectTo;
            res.writeHead(result.redirectTo ? 302 : 200, headers);
            res.end();
          } catch (err) {
            sendJson(statusOf(err), { error: messageOf(err), code: codeOf(err) });
          }
          return true;
        },
      },
      {
        name: "auth:me",
        method: "GET",
        match: "/api/auth/me",
        handler: async ({ req, sendJson }) => {
          const user = await auth.user({ req });
          const scopes = await auth.scopes({ req });
          sendJson(200, {
            authenticated: Boolean(user),
            authEnabled: auth.enabled,
            user: user ?? null,
            scopes,
            isAdmin: auth.adminScope ? scopes.includes(auth.adminScope) : false,
            adminScope: auth.adminScope ?? null,
          });
          return true;
        },
      },
      {
        name: "auth:backchannel-logout",
        method: "POST",
        match: "/api/auth/backchannel-logout",
        handler: async ({ req, res, sendJson }) => {
          const raw = await readBody(req);
          const token = readFormBody(raw).get("logout_token");
          if (!token) {
            sendJson(400, {
              error: 'Expected a form-encoded body containing a "logout_token" parameter.',
              code: "logout_missing_token",
            });
            return true;
          }
          try {
            const result = await auth.handleBackChannelLogout(token);
            // 200 with a count rather than 204: an operator debugging a misconfigured
            // authentik logout mapping needs to see that the token validated AND matched
            // zero sessions, which a 204 cannot distinguish from success.
            res.writeHead(200, { "Content-Type": "application/json", "Cache-Control": "no-store" });
            res.end(JSON.stringify({ ok: true, destroyed: result.destroyed }));
          } catch (err) {
            sendJson(statusOf(err), { error: messageOf(err), code: codeOf(err) });
          }
          return true;
        },
      },
    ],
    "core:auth",
  );
}

/**
 * Only same-origin, absolute-PATH return targets are accepted.
 *
 * `//evil.example` is rejected as well as `https://evil.example`: a protocol-relative URL
 * is a valid absolute URL to a different origin, and `new URL("//x", base)` would happily
 * resolve it there.
 */
export function sanitizeReturnTo(value: string | null | undefined): string | undefined {
  if (!value) return undefined;
  if (!value.startsWith("/")) return undefined;
  if (value.startsWith("//")) return undefined;
  return value;
}

function readBody(req: { on: (event: string, cb: (chunk: any) => void) => void }): Promise<string> {
  return new Promise((resolve, reject) => {
    let body = "";
    req.on("data", (chunk) => {
      body += typeof chunk === "string" ? chunk : chunk.toString("utf-8");
    });
    req.on("end", () => resolve(body));
    req.on("error", reject);
  });
}

export function statusOf(err: unknown): number {
  return err instanceof AuthError ? err.httpStatus : 500;
}

export function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function codeOf(err: unknown): string | undefined {
  return err instanceof AuthError ? err.code : undefined;
}
