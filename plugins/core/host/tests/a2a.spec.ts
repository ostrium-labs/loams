import { describe, it, expect, beforeEach, afterEach } from "vite-plus/test";
import { Service } from "cordis";
import { createHarness, manifest, type Harness } from "./helpers.js";
import {
  A2AError,
  A2AErrorCode,
  AGENT_CARD_PATH,
  buildAggregateCard,
  buildAgentCard,
  LEGACY_AGENT_CARD_PATH,
  mountA2A,
  parseSendRequest,
  sendMessage,
} from "../src/a2a.js";
import type { A2ADeps } from "../src/a2a.js";
import type { A2ATask } from "../src/types.js";
import type { PluginAgentSkill } from "../src/types.js";

/**
 * The A2A layer is the external contract, so these tests are written against the
 * wire shapes rather than against the internals: a card that is missing a field
 * a client reads, or a Task that comes back in a non-terminal state, breaks
 * callers without breaking anything internally.
 *
 * The routes are exercised over HTTP for the same reason the host tests are: the
 * claim under test is "a disabled agent is not addressable", and that is only
 * true if the route layer agrees with the registry.
 */

const SKILLS: PluginAgentSkill[] = [
  {
    id: "listChannels",
    name: "List channels",
    description: "List every channel the bot is in.",
    tags: ["read"],
    examples: ["listChannels"],
  },
  {
    id: "sendMessage",
    name: "Send message",
    description: "Post a message to a channel.",
  },
];

function agentManifest(id: string, overrides: Record<string, unknown> = {}) {
  return manifest({
    id,
    defaultEnabled: false,
    ...overrides,
  } as any) as any;
}

/** A manifest that declares an agent, which is the only kind a card can describe. */
function withAgent(id: string, overrides: Record<string, unknown> = {}): any {
  const base = agentManifest(id, overrides);
  return {
    ...base,
    agent: {
      name: `${base.name} Agent`,
      description: `The ${base.name} agent.`,
      version: "2.1.0",
      skills: SKILLS,
    },
  };
}

async function boot(h: Harness) {
  await (h.registry as any)[Service.init]();
  await (h.host as any)[Service.init]();
}

function deps(h: Harness): A2ADeps {
  return {
    registry: h.registry,
    bus: h.bus,
    ctx: h.ctx,
    baseUrl: () => h.baseUrl,
  };
}

/** `POST /a2a/v1/message:send` body, in the canonical data-part form. */
function sendBody(agent: string, skill: string, params: Record<string, unknown> = {}) {
  return {
    message: {
      kind: "message",
      id: "msg-1",
      role: "user",
      parts: [
        { kind: "text", text: "hello" },
        { kind: "data", data: { agent, skill, params } },
      ],
    },
  };
}

describe("a2a", () => {
  let h: Harness;

  beforeEach(async () => {
    h = await createHarness();
  });

  afterEach(async () => {
    await h.close();
  });

  /* ---------------------------------------------------------------------- */
  /* request parsing                                                        */
  /* ---------------------------------------------------------------------- */

  describe("parseSendRequest", () => {
    it("reads agent, skill and params out of a data part", () => {
      const parsed = parseSendRequest(sendBody("zulip", "listChannels", { limit: 5 }));
      expect(parsed.agentId).toBe("zulip");
      expect(parsed.skill).toBe("listChannels");
      expect(parsed.params).toEqual({ limit: 5 });
      expect(parsed.messageId).toBe("msg-1");
    });

    it("accepts the curl shorthand", () => {
      const parsed = parseSendRequest({
        message: { parts: [{ kind: "data", data: { agent: "a", skill: "s" } }] },
      });
      expect(parsed.agentId).toBe("a");
      expect(parsed.skill).toBe("s");
      expect(parsed.params).toEqual({});
    });

    it("falls back to message metadata when the data part carries no routing", () => {
      const parsed = parseSendRequest({
        message: {
          id: "m-9",
          parts: [{ kind: "text", text: "do the thing" }],
          metadata: { agent: "zulip", skill: "sendMessage", params: { text: "hi" } },
        },
      });
      expect(parsed.agentId).toBe("zulip");
      expect(parsed.skill).toBe("sendMessage");
      expect(parsed.params).toEqual({ text: "hi" });
      expect(parsed.messageId).toBe("m-9");
    });

    it("rejects a body with no parts", () => {
      expect(() => parseSendRequest({ message: { parts: [] } })).toThrow(A2AError);
      try {
        parseSendRequest({ message: { parts: [] } });
      } catch (err) {
        expect((err as A2AError).code).toBe(A2AErrorCode.InvalidRequest);
        expect((err as A2AError).httpStatus).toBe(400);
      }
    });

    it("rejects a body with no target agent", () => {
      try {
        parseSendRequest({ message: { parts: [{ kind: "text", text: "hi" }] } });
        expect.unreachable("a body with no agent must not parse");
      } catch (err) {
        expect((err as A2AError).code).toBe(A2AErrorCode.InvalidRequest);
      }
    });

    it("rejects a part that is not a well-formed A2A part", () => {
      try {
        parseSendRequest({ message: { parts: [{ kind: "video" }] } });
        expect.unreachable("an unknown part kind must not parse");
      } catch (err) {
        expect((err as A2AError).code).toBe(A2AErrorCode.InvalidRequest);
      }
    });
  });

  /* ---------------------------------------------------------------------- */
  /* agent cards                                                            */
  /* ---------------------------------------------------------------------- */

  describe("agent cards", () => {
    it("builds a per-agent card from the owning manifest", () => {
      const card = buildAgentCard(withAgent("zulip"), { baseUrl: "http://localhost:3001" });
      expect(card.name).toBe("zulip Agent");
      expect(card.version).toBe("2.1.0");
      expect(card.url).toBe("http://localhost:3001/a2a/v1/message:send");
      expect(card.protocolVersion).toBe("0.3.0");
      expect(card.capabilities).toEqual({
        streaming: false,
        pushNotifications: false,
        stateTransitionHistory: true,
      });
      // Per-agent cards keep the BARE skill id: the agent is already addressed by
      // the URL, so namespacing it here would make a client send "zulip.s".
      expect(card.skills.map((skill) => skill.id)).toEqual(["listChannels", "sendMessage"]);
    });

    it("namespaces skill ids in the aggregate card so two adapters cannot collide", () => {
      const card = buildAggregateCard(
        [
          withAgent("zulip"),
          {
            ...withAgent("matomo"),
            agent: {
              name: "Matomo Agent",
              description: "d",
              version: "1.0.0",
              skills: [{ id: "listChannels", name: "List channels", description: "d" }],
            },
          },
          { id: "chart-specs", name: "Chart specs", description: "no agent here" },
        ],
        { baseUrl: "http://localhost:3001" },
      );
      expect(card.skills.map((skill) => skill.id).sort()).toEqual([
        "matomo.listChannels",
        "zulip.listChannels",
        "zulip.sendMessage",
      ]);
      // A plugin with no agent block contributes nothing at all.
      expect(card.additionalInterfaces?.map((entry) => entry.url)).toEqual([
        "http://localhost:3001/.well-known/agent-card/zulip",
        "http://localhost:3001/.well-known/agent-card/matomo",
      ]);
    });

    it("lists only loaded agents on /.well-known/agent-card.json", async () => {
      h.registry.register(withAgent("zulip", { defaultEnabled: false }), {
        skills: () => [{ id: "listChannels", handle: () => ({ channels: [] }) }],
      });
      h.registry.register(withAgent("matomo", { defaultEnabled: true }), {
        skills: () => [{ id: "listChannels", handle: () => ({ sites: [] }) }],
      });
      h.registry.register(agentManifest("plain", { defaultEnabled: true }), {});
      mountA2A(deps(h), h.router);
      await boot(h);

      // `zulip` is registered but never enabled, so it must not be advertised --
      // advertising it would promise a `message:send` that answers 404.
      expect(h.host.isLoaded("matomo")).toBe(true);
      expect(h.host.isLoaded("zulip")).toBe(false);

      const response = await h.request("GET", "/.well-known/agent-card.json");
      expect(response.status).toBe(200);
      expect(response.body.skills.map((skill: any) => skill.id).sort()).toEqual([
        "matomo.listChannels",
        "matomo.sendMessage",
      ]);
      expect(response.body.additionalInterfaces).toEqual([
        { url: `${h.baseUrl}/.well-known/agent-card/matomo`, transport: "JSON" },
      ]);
      expect(response.body.protocolVersion).toBe("0.3.0");
    });

    it("serves the same card at the pre-1.0 path, byte for byte", async () => {
      // A2A v1.0 moved the card to `agent-card.json`. The old spelling stays
      // served so a client written against it keeps working, and it is served
      // from the same handler precisely so the two cannot drift.
      expect(AGENT_CARD_PATH).toBe("/.well-known/agent-card.json");
      expect(LEGACY_AGENT_CARD_PATH).toBe("/.well-known/agent.json");

      h.registry.register(withAgent("matomo", { defaultEnabled: true }), {
        skills: () => [{ id: "listChannels", handle: () => ({ sites: [] }) }],
      });
      mountA2A(deps(h), h.router);
      await boot(h);

      const canonical = await h.request("GET", AGENT_CARD_PATH);
      const legacy = await h.request("GET", LEGACY_AGENT_CARD_PATH);
      expect(canonical.status).toBe(200);
      expect(legacy.status).toBe(200);
      expect(legacy.body).toEqual(canonical.body);
    });

    it("serves a per-id card and refuses one for a disabled agent", async () => {
      h.registry.register(withAgent("zulip", { defaultEnabled: false }), {});
      mountA2A(deps(h), h.router);
      await boot(h);

      const off = await h.request("GET", "/.well-known/agent-card/zulip");
      expect(off.status).toBe(404);
      expect(off.body.error.code).toBe(A2AErrorCode.AgentNotLoaded);

      await h.host.enable("zulip");
      const on = await h.request("GET", "/.well-known/agent-card/zulip");
      expect(on.status).toBe(200);
      expect(on.body.skills).toHaveLength(2);
      expect(on.body.url).toBe(`${h.baseUrl}/a2a/v1/message:send`);
    });

    it("404s a card for an id that is not in the catalog at all", async () => {
      mountA2A(deps(h), h.router);
      await boot(h);

      const response = await h.request("GET", "/.well-known/agent-card/ghost");
      expect(response.status).toBe(404);
      expect(response.body.error.code).toBe(A2AErrorCode.NotFound);
    });
  });

  /* ---------------------------------------------------------------------- */
  /* message:send                                                           */
  /* ---------------------------------------------------------------------- */

  describe("message:send", () => {
    beforeEach(() => {
      h.registry.register(withAgent("zulip", { defaultEnabled: false }), {
        skills: () => [
          {
            id: "listChannels",
            handle: () => ({ channels: ["general", "random"] }),
          },
        ],
      });
      h.registry.register(agentManifest("plain", { defaultEnabled: false }), {});
    });

    it("returns a completed Task carrying the result as an Artifact", async () => {
      await boot(h);
      await h.host.enable("zulip");
      mountA2A(deps(h), h.router);

      const response = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("zulip", "listChannels"),
      );

      expect(response.status).toBe(200);
      const task = response.body as A2ATask;
      expect(task.kind).toBe("task");
      expect(task.status.state).toBe("completed");
      expect(typeof task.status.timestamp).toBe("string");

      expect(task.artifacts).toHaveLength(1);
      const artifact = task.artifacts![0];
      expect(artifact.name).toBe("listChannels-result");
      expect(artifact.parts).toEqual([
        {
          kind: "data",
          data: { channels: ["general", "random"] },
        },
      ]);

      // History keeps both sides of the conversation.
      expect(task.history!.map((message) => message.role)).toEqual(["user", "agent"]);
      expect(task.history![0].parts).toEqual([
        { kind: "text", text: "hello" },
        { kind: "data", data: { agent: "zulip", skill: "listChannels", params: {} } },
      ]);
    });

    it("routes a message to the handler whose skill id matches", async () => {
      // Every skill of a plugin shares one agent id, so this is the test that
      // fails if the host stops filtering handlers by `message.skill`: without
      // the filter, `bus.request` returns the FIRST handler's reply, which is
      // whatever the plugin listed first rather than what was asked for.
      h.registry.unregister("zulip");
      h.registry.register(withAgent("zulip", { defaultEnabled: false }), {
        skills: () => [
          { id: "listChannels", handle: () => ({ wrong: "listChannels" }) },
          { id: "sendMessage", handle: () => ({ right: "sendMessage" }) },
        ],
      });
      await boot(h);
      await h.host.enable("zulip");
      mountA2A(deps(h), h.router);

      const response = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("zulip", "sendMessage"),
      );
      expect(response.status).toBe(200);
      expect(response.body.status.state).toBe("completed");
      expect(response.body.artifacts[0].parts).toEqual([
        { kind: "data", data: { right: "sendMessage" } },
      ]);
    });

    it("records a failed skill as a failed Task rather than an HTTP error", async () => {
      await boot(h);
      // Re-register rather than mutate the loader: the registry has no replace.
      h.registry.unregister("zulip");
      h.registry.register(withAgent("zulip", { defaultEnabled: false }), {
        skills: () => [
          {
            id: "listChannels",
            handle: () => {
              throw new Error("upstream is down");
            },
          },
        ],
      });
      await h.host.enable("zulip");
      mountA2A(deps(h), h.router);

      const response = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("zulip", "listChannels"),
      );

      // A2A reports a skill failure as a terminal Task; only protocol and routing
      // problems are HTTP errors.
      expect(response.status).toBe(200);
      expect(response.body.status.state).toBe("failed");
      expect(response.body.status.message.parts[0].text).toContain("upstream is down");
      expect(response.body.artifacts).toBeUndefined();
    });

    it("rejects an undeclared skill with an A2A error", async () => {
      await boot(h);
      await h.host.enable("zulip");
      mountA2A(deps(h), h.router);

      const response = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("zulip", "explode"),
      );

      expect(response.status).toBe(404);
      expect(response.body.error.code).toBe(A2AErrorCode.NotFound);
      expect(response.body.error.data.validSkills).toEqual(["listChannels", "sendMessage"]);
    });

    it("rejects a disabled agent with an A2A error", async () => {
      await boot(h);
      mountA2A(deps(h), h.router);

      const response = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("zulip", "listChannels"),
      );

      expect(response.status).toBe(404);
      expect(response.body.error.code).toBe(A2AErrorCode.AgentNotLoaded);
      expect(response.body.error.data).toMatchObject({ agent: "zulip" });
    });

    it("rejects an agent that is not in the catalog, listing what IS loaded", async () => {
      await boot(h);
      mountA2A(deps(h), h.router);

      const response = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("ghost", "listChannels"),
      );

      expect(response.status).toBe(404);
      expect(response.body.error.code).toBe(A2AErrorCode.NotFound);
      expect(response.body.error.data.agents).toEqual([]);
    });

    it("stops answering the moment the plugin is unloaded", async () => {
      await boot(h);
      await h.host.enable("zulip");
      mountA2A(deps(h), h.router);

      const before = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("zulip", "listChannels"),
      );
      expect(before.status).toBe(200);

      await h.host.disable("zulip");
      const after = await h.request(
        "POST",
        "/a2a/v1/message:send",
        sendBody("zulip", "listChannels"),
      );
      expect(after.status).toBe(404);
      expect(after.body.error.code).toBe(A2AErrorCode.AgentNotLoaded);
    });

    it("returns 400 for a body it cannot parse", async () => {
      await boot(h);
      mountA2A(deps(h), h.router);

      const response = await h.request("POST", "/a2a/v1/message:send", { nope: true });

      expect(response.status).toBe(400);
      expect(response.body.error.code).toBe(A2AErrorCode.InvalidRequest);
    });

    it("unregisters its routes when the mount is disposed", async () => {
      await boot(h);
      await h.host.enable("zulip");
      const remove = mountA2A(deps(h), h.router);

      expect((await h.request("GET", "/.well-known/agent.json")).status).toBe(200);
      remove();
      expect((await h.request("GET", "/.well-known/agent.json")).status).toBe(404);
    });
  });

  /* ---------------------------------------------------------------------- */
  /* the pure entry point                                                   */
  /* ---------------------------------------------------------------------- */

  it("sendMessage throws rather than returning a Task when routing is impossible", async () => {
    h.registry.register(agentManifest("plain", { defaultEnabled: false }), {});
    await boot(h);

    await expect(sendMessage(deps(h), sendBody("plain", "anything"))).rejects.toBeInstanceOf(
      A2AError,
    );
  });
});
