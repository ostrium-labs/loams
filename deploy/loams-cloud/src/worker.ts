// The Worker's entry point: Cloudflare and npm imports live only here, so the
// rest of src/ runs under Node's tests. Not deployed (plan LC1, LC2a; the
// owner's setup steps come first). Built with wrangler 4.149.0.

import { DurableObject } from "cloudflare:workers";
import { connect } from "@tidbcloud/serverless";
import admins from "../config/admins.toml";
import migration0001 from "../schema/0001_waitlist.sql";
import disposable from "../policy/disposable_email_blocklist.conf";
import overrides from "../policy/email-domains.toml";
import { build, controlDbHandler, fetchHandler, rateLimiterHandler, scheduled, type Built, type Connect, type DoState, type Env } from "./entry.ts";

const files = { admins, disposable, overrides, migrations: [{ version: 1, name: "0001_waitlist", text: migration0001 }] };

let built: Promise<Built> | null = null;
function get(env: Env): Promise<Built> {
  built ??= build(env, files).catch((e) => {
    built = null;
    throw e;
  });
  return built;
}

export class ControlDb extends DurableObject<Env> {
  private readonly handler: (req: Request) => Promise<Response>;
  constructor(state: DurableObjectState, env: Env) {
    super(state, env);
    this.handler = controlDbHandler(env, connect as unknown as Connect);
  }
  fetch(req: Request): Promise<Response> {
    return this.handler(req);
  }
}

export class RateLimiter extends DurableObject<Env> {
  private readonly handler: (req: Request) => Promise<Response>;
  constructor(state: DurableObjectState, env: Env) {
    super(state, env);
    this.handler = rateLimiterHandler(state as unknown as DoState);
  }
  fetch(req: Request): Promise<Response> {
    return this.handler(req);
  }
  async alarm(): Promise<void> {
    await this.ctx.storage.deleteAll();
  }
}

export default {
  fetch(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    return fetchHandler(() => get(env))(req, ctx);
  },
  async scheduled(event: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    ctx.waitUntil(get(env).then((b) => scheduled(event.cron, b)).then((r) => console.log(JSON.stringify({ event: "cron", cron: event.cron, ...r }))));
  },
};
