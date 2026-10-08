import { describe, it, expect, vi, beforeEach } from "vite-plus/test";
import { Context } from "cordis";
import { BiService } from "../src/service.js";

describe("BiService", () => {
  let ctx: Context;
  let service: BiService;

  beforeEach(() => {
    ctx = new Context();
    service = new BiService(ctx, {
      baseUrl: "http://localhost:8088",
      username: "admin",
      password: "admin_password",
    });
    vi.stubGlobal("fetch", vi.fn());
  });

  it("_ensureAuth caches token and reuses within expiry", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce({
      ok: true,
      json: async () => ({ access_token: "token1" }),
    } as any);

    const token1 = await service._ensureAuth();
    expect(token1).toBe("token1");
    expect(fetchMock).toHaveBeenCalledTimes(1);

    const token2 = await service._ensureAuth();
    expect(token2).toBe("token1");
    expect(fetchMock).toHaveBeenCalledTimes(1); // Not called again
  });

  it("listDatasets calls correct URL", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ access_token: "token1" }),
      } as any) // auth
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ result: [] }),
      } as any); // listDatasets

    await service.listDatasets();
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "http://localhost:8088/api/v1/dataset/?q=(page_size:100)",
      expect.any(Object),
    );
  });

  it("describeDataset caches result after first call", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ access_token: "token1" }),
      } as any) // auth
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ result: { id: 1, table_name: "test" } }),
      } as any); // describeDataset

    const result1 = await service.describeDataset(1);
    expect(result1.id).toBe(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);

    const result2 = await service.describeDataset(1);
    expect(result2.id).toBe(1);
    expect(fetchMock).toHaveBeenCalledTimes(2); // Cached
  });

  it("queryData sends correct POST payload", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ access_token: "token1" }),
      } as any) // auth
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ result: "csrf" }),
      } as any) // csrf
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ data: [] }),
      } as any); // queryData

    await service.queryData(1, ["col1"], [], [], 100);
    expect(fetchMock).toHaveBeenNthCalledWith(
      3,
      "http://localhost:8088/api/v1/chart/data",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          datasource: { id: 1, type: "table" },
          queries: [{ columns: ["col1"], filters: [], orderby: [], row_limit: 100 }],
        }),
      }),
    );
  });

  it("_fetch throws on non-OK response", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ access_token: "token1" }),
      } as any) // auth
      .mockResolvedValueOnce({
        ok: false,
        status: 500,
        statusText: "Internal Server Error",
      } as any); // fetch

    await expect(service._fetch("/api/v1/dataset/")).rejects.toThrow(
      "Request failed: 500 Internal Server Error",
    );
  });
});
