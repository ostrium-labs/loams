import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

describe("readEnvWithFallback", () => {
  let write: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.resetModules();
    delete process.env.LOAMS_BI_URL;
    delete process.env.SUPERSET_URL;
    delete process.env.LOAMS_BI_PORT;
    delete process.env.SUPERSET_PORT;
    write = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });

  afterEach(() => {
    write.mockRestore();
    delete process.env.LOAMS_BI_URL;
    delete process.env.SUPERSET_URL;
    delete process.env.LOAMS_BI_PORT;
    delete process.env.SUPERSET_PORT;
  });

  it("prefers the new name and does not warn", async () => {
    const { readEnvWithFallback } = await import("../src/env.js");
    process.env.LOAMS_BI_URL = "http://new";
    process.env.SUPERSET_URL = "http://old";
    expect(readEnvWithFallback("LOAMS_BI_URL", "SUPERSET_URL")).toBe("http://new");
    expect(write).not.toHaveBeenCalled();
  });

  it("falls back to the old name", async () => {
    const { readEnvWithFallback } = await import("../src/env.js");
    process.env.SUPERSET_URL = "http://old";
    expect(readEnvWithFallback("LOAMS_BI_URL", "SUPERSET_URL")).toBe("http://old");
  });

  it("returns undefined when neither is set", async () => {
    const { readEnvWithFallback } = await import("../src/env.js");
    expect(readEnvWithFallback("LOAMS_BI_URL", "SUPERSET_URL")).toBeUndefined();
    expect(write).not.toHaveBeenCalled();
  });

  it("warns exactly once per old variable", async () => {
    const { readEnvWithFallback } = await import("../src/env.js");
    process.env.SUPERSET_URL = "http://old";
    process.env.SUPERSET_PORT = "9000";
    readEnvWithFallback("LOAMS_BI_URL", "SUPERSET_URL");
    readEnvWithFallback("LOAMS_BI_URL", "SUPERSET_URL");
    expect(write).toHaveBeenCalledTimes(1);
    readEnvWithFallback("LOAMS_BI_PORT", "SUPERSET_PORT");
    readEnvWithFallback("LOAMS_BI_PORT", "SUPERSET_PORT");
    expect(write).toHaveBeenCalledTimes(2);
    expect(String(write.mock.calls[0][0])).toContain("SUPERSET_URL");
  });
});
