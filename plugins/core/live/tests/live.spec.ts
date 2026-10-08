import { describe, it, expect } from "vite-plus/test";
import { FrameReader } from "../src/connect.js";
import { apply, initialState, reduce } from "../src/store.js";

/** Frames a payload the way the Connect server does. */
function frame(payload: unknown, flags = 0): Uint8Array {
  const body = new TextEncoder().encode(JSON.stringify(payload));
  const out = new Uint8Array(5 + body.length);
  new DataView(out.buffer).setUint32(1, body.length, false);
  out[0] = flags;
  out.set(body, 5);
  return out;
}

describe("FrameReader", () => {
  it("reads one frame", () => {
    const reader = new FrameReader();
    expect(reader.push(frame({ cursor: "c1" }))).toEqual([
      { end: false, payload: { cursor: "c1" } },
    ]);
  });

  it("reads several frames out of one chunk", () => {
    const reader = new FrameReader();
    const a = frame({ cursor: "c1" });
    const b = frame({ cursor: "c2" });
    const merged = new Uint8Array(a.length + b.length);
    merged.set(a, 0);
    merged.set(b, a.length);
    expect(reader.push(merged)).toHaveLength(2);
  });

  it("waits for the rest of a frame split across chunks", () => {
    // The whole reason this is a class and not an inline parse: a snapshot
    // arriving in two TCP reads must not be read as a truncated message.
    const reader = new FrameReader();
    const whole = frame({ cursor: "c1", snapshot: { operations: [{ id: "op1" }] } });
    const cut = Math.floor(whole.length / 2);

    expect(reader.push(whole.subarray(0, cut))).toEqual([]);
    expect(reader.push(whole.subarray(cut))).toEqual([
      {
        end: false,
        payload: { cursor: "c1", snapshot: { operations: [{ id: "op1" }] } },
      },
    ]);
  });

  it("marks the end-of-stream frame", () => {
    const reader = new FrameReader();
    expect(reader.push(frame({}, 0x02))).toEqual([{ end: true, payload: {} }]);
  });

  it("reads an error carried on the end frame", () => {
    const reader = new FrameReader();
    const frames = reader.push(frame({ error: { code: "unauthenticated", message: "no" } }, 0x02));
    expect(frames[0].end).toBe(true);
    expect(frames[0].payload.error).toEqual({ code: "unauthenticated", message: "no" });
  });
});

describe("reduce", () => {
  it("replaces everything on a snapshot", () => {
    let state = apply(initialState(), {
      kind: "snapshot",
      cursor: "c1",
      items: [{ id: "op1" }, { id: "op2" }],
    });
    state = apply(state, { kind: "remove", id: "op1", cursor: "c1" });
    state = apply(state, { kind: "snapshot", cursor: "c2", items: [{ id: "op9" }] });
    expect(state.items).toEqual([{ id: "op9" }]);
  });

  it("adds then updates in place on upsert", () => {
    let state = apply(initialState(), { kind: "upsert", cursor: "c1", item: { id: "op1", done: 1 } });
    state = apply(state, { kind: "upsert", cursor: "c1", item: { id: "op1", done: 2 } });
    expect(state.items).toEqual([{ id: "op1", done: 2 }]);
  });

  it("treats a heartbeat as proof of life, not as a change", () => {
    // The console's inbox rule: a heartbeat must not clear a stale warning or
    // count as a revision, or every tile re-renders 4x a minute for nothing.
    // It may still advance the cursor -- that is real progress.
    const state = { ...initialState(), error: "stream down", revision: 7 };
    const next = reduce(state, { cursor: "c9", heartbeat: {} })!;
    expect(next.revision).toBe(7);
    expect(next.error).toBe("stream down");
    expect(next.cursor).toBe("c9");
  });

  it("moves the cursor on a heartbeat that carries a new one", () => {
    const state = { ...initialState(), cursor: "c1" };
    const next = reduce(state, { cursor: "c2", heartbeat: {} })!;
    expect(next.cursor).toBe("c2");
    expect(next.revision).toBe(state.revision);
  });

  it("ignores a heartbeat that repeats the cursor", () => {
    const state = { ...initialState(), cursor: "c1" };
    expect(reduce(state, { cursor: "c1", heartbeat: {} })).toBeUndefined();
  });

  it("ignores a remove for something already gone", () => {
    const state = apply(initialState(), { kind: "snapshot", cursor: "c1", items: [{ id: "op1" }] });
    expect(reduce(state, { cursor: "c1", remove: { id: "nope" } })).toBeUndefined();
  });

  it("flattens a record so widgets can read its fields as columns", () => {
    const state = apply(initialState(), {
      kind: "upsert",
      cursor: "c1",
      item: { id: "op1", kind: "collection.import", progress: null } as any,
    });
    // `progress` present as an explicit null, not dropped: a chart column that
    // vanishes between rows reads as "no value" rather than "not applicable".
    expect(state.items[0]).toEqual({ id: "op1", kind: "collection.import", progress: null });
  });

  it("drops a record with no id rather than rendering a broken node", () => {
    expect(reduce(initialState(), { cursor: "c1", upsert: { kind: "x" } })).toBeUndefined();
  });

  it("takes the first list a snapshot carries", () => {
    const state = reduce(initialState(), {
      cursor: "c1",
      snapshot: { operations: [{ id: "op1" }] },
    })!;
    expect(state.items).toEqual([{ id: "op1" }]);
  });

  it("counts a revision only on real changes", () => {
    let state = apply(initialState(), { kind: "snapshot", cursor: "c1", items: [{ id: "op1" }] });
    expect(state.revision).toBe(1);
    state = apply(state, { kind: "cursor", cursor: "c2" });
    expect(state.revision).toBe(1);
    state = apply(state, { kind: "upsert", cursor: "c3", item: { id: "op2" } });
    expect(state.revision).toBe(2);
  });
});