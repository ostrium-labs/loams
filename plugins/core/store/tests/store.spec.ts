import { describe, it, expect, vi, beforeEach } from "vite-plus/test";
import { Context, Service } from "cordis";
import { StoreService } from "../src/service.js";

vi.mock("pg", () => {
  const query = vi
    .fn()
    .mockResolvedValue({ rowCount: 1, rows: [{ spec: { id: "dash-1", version: 1 } }] });
  const release = vi.fn();
  const connect = vi.fn().mockResolvedValue({ query, release });
  const Pool = vi.fn().mockImplementation(function (this: any) {
    this.query = query;
    this.connect = connect;
    return this;
  });
  return { Pool };
});

describe("StoreService", () => {
  let ctx: Context;
  let store: StoreService;
  let poolMock: any;
  let clientMock: any;

  beforeEach(async () => {
    ctx = new Context();
    store = new StoreService(ctx, { connectionString: "postgres://localhost/test" });
    await (store as any)[Service.init]();

    // reset mocks to clean state after init
    poolMock = store.pool;
    clientMock = await poolMock.connect();

    vi.clearAllMocks();
    poolMock.query.mockResolvedValue({ rowCount: 1, rows: [] });
    clientMock.query.mockResolvedValue({ rowCount: 1, rows: [] });
  });

  it("should initialize and run ensureSchema", async () => {
    const ctx2 = new Context();
    const store2 = new StoreService(ctx2, { connectionString: "test" });
    store2._ensureSchema = vi.fn();
    await (store2 as any)[Service.init]();
    expect(store2._ensureSchema).toHaveBeenCalled();
  });

  it("saveDashboard runs correct SQL in a transaction", async () => {
    const spec = { id: "dash-1", version: 2 };
    await store.saveDashboard(spec, "user1", { op: "replace" });

    expect(clientMock.query).toHaveBeenCalledWith("BEGIN");
    expect(clientMock.query).toHaveBeenCalledWith(
      expect.stringContaining("INSERT INTO dashboards"),
      [spec.id, spec.version, spec],
    );
    expect(clientMock.query).toHaveBeenCalledWith(
      expect.stringContaining("INSERT INTO dashboard_versions"),
      [spec.id, spec.version, spec, { op: "replace" }, "user1"],
    );
    expect(clientMock.query).toHaveBeenCalledWith(
      expect.stringContaining("INSERT INTO audit_log"),
      [spec.id, "SAVE", "user1", { op: "replace" }],
    );
    expect(clientMock.query).toHaveBeenCalledWith("COMMIT");
    expect(clientMock.release).toHaveBeenCalled();
  });

  it("getDashboard throws on not found", async () => {
    poolMock.query.mockResolvedValueOnce({ rowCount: 0, rows: [] });
    await expect(store.getDashboard("missing-id")).rejects.toThrow(
      "Dashboard not found: missing-id",
    );
  });

  it("listDashboards returns correct shape", async () => {
    poolMock.query.mockResolvedValueOnce({
      rowCount: 1,
      rows: [{ id: "dash-1", title: "Test Dash", version: 1 }],
    });
    const res = await store.listDashboards();
    expect(res).toEqual([{ id: "dash-1", title: "Test Dash", version: 1 }]);
  });
});
