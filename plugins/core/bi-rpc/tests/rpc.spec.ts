import { describe, it, expect } from "vite-plus/test";
import { toJson, fromJson } from "@bufbuild/protobuf";
import { ValueSchema, type Value as StructValueMessage } from "@bufbuild/protobuf/wkt";
import {
  createDataService,
  createDashboardService,
  createThemeService,
  type RpcContext,
} from "../src/index.js";
import { RowSchema } from "../src/gen/bi/v1/data_pb.js";

/**
 * These tests pin the transport behaviour that is easy to get silently wrong:
 *
 *  - Column values keep their JSON type across the wire. A `map<string, double>`
 *    would coerce `true` and `"2024-01-01"` into numbers and quietly corrupt a
 *    dashboard, so `Value` is a typed oneof and this asserts every branch.
 *  - The several different response envelopes Superset returns are all unwrapped
 *    (`{result:[...]}`, `{result:[{data:[...]}]}`, a bare array).
 *  - A missing cordis port produces a message naming the port, not a crash deep
 *    inside protobuf serialization.
 */

function makeCtx(overrides: Partial<RpcContext> = {}): RpcContext {
  const noop = () => {};
  return {
    controlPlane: {
      listDatasets: async () => ({ result: [{ id: 1, table_name: "sales" }] }),
      describeDataset: async () => ({
        id: 7,
        table_name: "sales",
        columns: [{ column_name: "amount", type: "BIGINT", is_dttm: false }],
      }),
      queryData: async () => ({ data: [] }),
    },
    dashboard: {
      listDashboards: async () => [],
      load: async () => ({ id: "d1" }),
      create: async (title: string) => ({ id: "d1", title }),
      patch: async () => ({ id: "d1" }),
    },
    data: { fetchWidgetData: async () => ({ rowcount: 0, data: [] }) },
    render: { compileWidget: async () => ({ series: [] }) },
    flint: {
      listThemes: () => [],
      resolveTheme: () => ({ valid: true, report: [] }),
    },
    logger: { info: noop, warn: noop, error: noop },
    ...overrides,
  };
}

// The individual factories are used directly rather than a name-keyed record:
// indexing such a record yields a UNION of the three ServiceImpl types, so
// `services(ctx).data.query(...)` stops typechecking. Use `registerRpcServices`
// for router wiring.
function services(ctx: RpcContext) {
  return {
    data: createDataService(ctx),
    dashboard: createDashboardService(ctx),
    theme: createThemeService(ctx),
  };
}

/** Read a row's columns back as plain JSON, exactly as a Connect client would. */
function rowValues(row: { values: Record<string, StructValueMessage> }): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(row.values).map(([key, v]) => [key, toJson(ValueSchema, v)]),
  );
}

describe("DataService.Query value encoding", () => {
  it("preserves number, string, boolean, and null column types", async () => {
    const ctx = makeCtx({
      controlPlane: {
        ...makeCtx().controlPlane,
        queryData: async () => ({
          rowcount: 1,
          data: [{ n: 42, s: "hello", b: true, nul: null }],
        }),
      },
    });
    const res = await services(ctx).data.query({ datasetId: 1n } as never, {} as never);
    expect(res.rowcount).toBe(1);
    expect(rowValues((res.rows ?? [])[0] as never)).toEqual({
      n: 42,
      s: "hello",
      b: true,
      nul: null,
    });
  });

  it("serializes a Date column as its ISO string rather than a number", async () => {
    const ctx = makeCtx({
      controlPlane: {
        ...makeCtx().controlPlane,
        queryData: async () => ({ data: [{ when: new Date("2024-03-01T00:00:00.000Z") }] }),
      },
    });
    const res = await services(ctx).data.query({ datasetId: 1n } as never, {} as never);
    expect(rowValues((res.rows ?? [])[0] as never).when).toBe("2024-03-01T00:00:00.000Z");
  });

  it("preserves a nested JSON column as JSON instead of stringifying it", async () => {
    const ctx = makeCtx({
      controlPlane: {
        ...makeCtx().controlPlane,
        queryData: async () => ({ data: [{ agg: { sum: 3 } }] }),
      },
    });
    const res = await services(ctx).data.query({ datasetId: 1n } as never, {} as never);
    expect(rowValues((res.rows ?? [])[0] as never).agg).toEqual({ sum: 3 });
  });

  it("unwraps the { result: [{ data }] } envelope", async () => {
    const ctx = makeCtx({
      controlPlane: {
        ...makeCtx().controlPlane,
        queryData: async () => ({ result: [{ data: [{ a: 1 }, { a: 2 }] }] }),
      },
    });
    const res = await services(ctx).data.query({ datasetId: 1n } as never, {} as never);
    expect(res.rows ?? []).toHaveLength(2);
  });

  it("unwraps a bare array payload", async () => {
    const ctx = makeCtx({
      controlPlane: {
        ...makeCtx().controlPlane,
        queryData: async () => [{ a: 1 }],
      },
    });
    const res = await services(ctx).data.query({ datasetId: 1n } as never, {} as never);
    expect(res.rows ?? []).toHaveLength(1);
  });

  it("tolerates an empty result", async () => {
    const res = await services(makeCtx()).data.query({ datasetId: 1n } as never, {} as never);
    expect(res.rows ?? []).toHaveLength(0);
    expect(res.rowcount).toBe(0);
  });
});

describe("DataService dataset listing", () => {
  it("unwraps the { result } envelope and counts", async () => {
    const res = await services(makeCtx()).data.listDatasets({} as never, {} as never);
    expect(res.count).toBe(1);
    expect(Number((res.datasets ?? [])[0]!.id)).toBe(1);
    expect((res.datasets ?? [])[0]!.tableName).toBe("sales");
  });

  it("maps dataset columns including the datetime flag", async () => {
    const res = await services(makeCtx()).data.describeDataset({ id: 7n } as never, {} as never);
    expect(Number(res.id)).toBe(7);
    expect((res.columns ?? [])[0]!.columnName).toBe("amount");
    expect((res.columns ?? [])[0]!.isDttm).toBe(false);
  });
});

describe("ThemeService", () => {
  it("returns the catalogue with icons", async () => {
    const ctx = makeCtx({
      flint: {
        listThemes: () => [
          { id: "economist", label: "Economist", description: "d", icon: "<svg/>" },
        ],
        resolveTheme: () => ({ valid: true, report: [] }),
      },
    });
    const res = await services(ctx).theme.listThemes({} as never, {} as never);
    expect(res.themes ?? []).toHaveLength(1);
    expect((res.themes ?? [])[0]!.icon).toBe("<svg/>");
  });

  it("treats a resolved-but-downgraded theme as valid, with its report", async () => {
    const ctx = makeCtx({
      flint: {
        listThemes: () => [],
        resolveTheme: () => ({
          valid: true,
          spec: { ink: { text: { primary: "#000" } } },
          report: [{ stage: "ground", path: "ink.series", message: "approximated" }],
        }),
      },
    });
    const res = await services(ctx).theme.getTheme({ id: "economist" } as never, {} as never);
    expect(res.valid).toBe(true);
    expect(res.report ?? []).toHaveLength(1);
    expect((res.report ?? [])[0]!.path).toBe("ink.series");
    expect(res.spec).toBeDefined();
  });

  it("returns valid:false and no spec for an unresolvable theme", async () => {
    const ctx = makeCtx({
      flint: {
        listThemes: () => [],
        resolveTheme: () => ({
          valid: false,
          report: [{ stage: "ground", path: "theme.preset", message: "unknown preset" }],
        }),
      },
    });
    const res = await services(ctx).theme.getTheme({ id: "nope" } as never, {} as never);
    expect(res.valid).toBe(false);
    expect(res.spec).toBeUndefined();
    expect((res.report ?? [])[0]!.message).toBe("unknown preset");
  });
});

describe("missing cordis ports", () => {
  it("names the missing port instead of throwing an opaque serialization error", async () => {
    const ctx = makeCtx();
    // Simulate a container where the render plugin failed to load.
    delete (ctx as Partial<RpcContext>).render;
    await expect(services(ctx).dashboard.previewWidget({} as never, {} as never)).rejects.toThrow(
      /ctx\.render/,
    );
  });
});

describe("Row/Value wire shape", () => {
  it("round-trips a value through JSON encoding", () => {
    expect(RowSchema.typeName).toBe("bi.v1.Row");
    const v = fromJson(ValueSchema, null as never);
    expect(toJson(ValueSchema, v)).toBeNull();
  });
});
