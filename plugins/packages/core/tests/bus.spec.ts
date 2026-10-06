import { describe, it, expect, beforeEach } from "vite-plus/test";
import { Context } from "cordis";
import { AgentBus, AgentNotLoadedError, AgentTimeoutError, type AgentHandler } from "../src/bus.js";
import type { AgentMessage } from "../src/types.js";

/**
 * The bus is where "one broken plugin must not take down the server" is either
 * true or not, so most of these cases are about isolation rather than delivery:
 * a throwing subscriber, an agent that is simply not there, a broadcast that
 * comes back around.
 */

function message(overrides: Partial<AgentMessage> = {}): AgentMessage {
  return {
    from: "caller",
    messageId: "m-1",
    skill: "ping",
    timestamp: Date.now(),
    ...overrides,
  };
}

describe("AgentBus", () => {
  let ctx: Context;
  let bus: AgentBus;

  beforeEach(() => {
    ctx = new Context();
    bus = new AgentBus(ctx, { defaultTimeoutMs: 200 });
  });

  it("fans a broadcast out to every subscriber", async () => {
    const seen: string[] = [];
    bus.subscribe("a", () => void seen.push("a"));
    bus.subscribe("b", () => void seen.push("b"));

    const result = await bus.publish(message({ skill: "news" }));

    expect(seen.sort()).toEqual(["a", "b"]);
    expect(result.errors).toEqual([]);
    expect(result.replies).toEqual([]);
  });

  it("delivers an addressed message only to its target", async () => {
    const seen: string[] = [];
    bus.subscribe("a", () => void seen.push("a"));
    bus.subscribe("b", () => void seen.push("b"));

    await bus.publish(message({ to: "b" }));

    expect(seen).toEqual(["b"]);
  });

  it("passes through a handler reply that is already an AgentMessage", async () => {
    bus.subscribe("echo", (incoming: AgentMessage) => ({
      ...incoming,
      from: "echo",
      to: undefined,
      text: `echo:${incoming.skill}`,
      timestamp: Date.now(),
    }));
    const reply = await bus.request(message({ to: "echo", skill: "hello" }));
    expect(reply).toMatchObject({ from: "echo", skill: "hello", text: "echo:hello" });
  });

  it("normalizes a bare return value into a message carrying data", async () => {
    bus.subscribe("calc", () => ({ answer: 42 }));
    const reply = await bus.request(message({ to: "calc" }));
    expect(reply.data).toEqual({ answer: 42 });
  });

  it("treats a lone { text } result as data, not as an envelope", async () => {
    // A skill that returns `{ text: "..." }` without a messageId is returning
    // structured data. Guessing otherwise would split a perfectly ordinary
    // result object across the A2A envelope.
    bus.subscribe("plain", () => ({ text: "just data" }));
    const reply = await bus.request(message({ to: "plain" }));
    expect(reply.data).toEqual({ text: "just data" });
    expect(reply.text).toBeUndefined();
  });

  it("times out instead of hanging forever", async () => {
    bus.subscribe("slow", () => new Promise(() => {}));
    await expect(bus.request(message({ to: "slow" }), 30)).rejects.toBeInstanceOf(
      AgentTimeoutError,
    );
  });

  it("reports an addressed-but-absent agent as not loaded", async () => {
    await expect(bus.request(message({ to: "ghost" }))).rejects.toBeInstanceOf(AgentNotLoadedError);
    await expect(bus.publish(message({ to: "ghost" }))).rejects.toThrow(
      'agent "ghost" is not loaded',
    );
  });

  it("stops delivering to one subscriber when another throws", async () => {
    const seen: string[] = [];
    const thrower: AgentHandler = () => {
      throw new Error("bad handler");
    };
    bus.subscribe("bad", thrower);
    bus.subscribe("good", () => void seen.push("good"));

    const result = await bus.publish(message());

    expect(seen).toEqual(["good"]);
    expect(result.errors).toHaveLength(1);
    expect((result.errors[0].error as Error).message).toBe("bad handler");
  });

  it("re-raises the subscriber error from request, but only after the others ran", async () => {
    bus.subscribe("bad", () => {
      throw new Error("bad handler");
    });
    bus.subscribe("good", () => "fine");

    await expect(bus.request(message({ to: "bad" }))).rejects.toThrow("bad handler");
  });

  it("does not leak a subscriber after unsubscribe", async () => {
    const calls: string[] = [];
    const off = bus.subscribe("a", () => void calls.push("hit"));
    expect(bus.has("a")).toBe(true);

    off();
    off(); // idempotent
    expect(bus.has("a")).toBe(false);
    expect(bus.agents()).toEqual([]);

    // The agent is gone as far as the bus is concerned, so an addressed message
    // fails the same way a never-loaded plugin does rather than being swallowed
    // as a broadcast nobody heard.
    await expect(bus.publish(message({ to: "a" }))).rejects.toBeInstanceOf(AgentNotLoadedError);
    expect(calls).toEqual([]);
  });

  it("stops a broadcast loop with the hop cap", async () => {
    // A second bus needs its own context: cordis refuses two services with the
    // same key on one container.
    const loopCtx = new Context();
    const looping = new AgentBus(loopCtx, { maxHops: 3, defaultTimeoutMs: 500 });
    let calls = 0;

    const replyToTheOther =
      (peer: string): AgentHandler =>
      (incoming) => {
        calls += 1;
        // Deliberately re-address the message to the other agent, which re-addresses
        // it back. Without a depth cap this never terminates.
        // The chain never produces a reply, so each request rejects once the hop
        // cap drops it. That rejection is the loop's actual terminator.
        void looping
          .request(message({ to: peer, skill: `hop-${calls}` }), 500)
          .catch(() => undefined);
        return undefined;
      };

    looping.subscribe("a", replyToTheOther("b"));
    looping.subscribe("b", replyToTheOther("a"));

    void looping.publish(message({ to: "a", skill: "hop-0" }));
    await new Promise((resolve) => setTimeout(resolve, 150));

    expect(calls).toBeGreaterThan(0);
    expect(calls).toBeLessThan(20);
  });

  it("lets a handler unsubscribe itself mid-flight without breaking delivery", async () => {
    let off = () => {};
    const calls: string[] = [];
    off = bus.subscribe("self-removing", () => {
      calls.push("first");
      off();
      return "done";
    });

    await expect(bus.request(message({ to: "self-removing" }))).resolves.toMatchObject({
      data: "done",
    });
    expect(calls).toEqual(["first"]);
    expect(bus.has("self-removing")).toBe(false);
  });
});
