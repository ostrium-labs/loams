import { describe, it, expect } from "vite-plus/test";
import { mapColumnsToSemanticTypes } from "../src/type-mapper.js";
import type { SupersetColumn } from "@loams-core/types";

/**
 * Superset's dataset API always returns `filterable` and `groupby` alongside the
 * name and type, and `SupersetColumn` requires them. These fixtures spell that
 * out rather than weakening the type, because the mapper never reads them and a
 * cast here would hide a future field going missing.
 */
function column(column_name: string, type: string, is_dttm = false): SupersetColumn {
  return { column_name, type, is_dttm, filterable: true, groupby: true };
}

describe("type-mapper", () => {
  it("Test TIMESTAMP column -> DateTime", () => {
    const col = column("created_at", "TIMESTAMP");
    const res = mapColumnsToSemanticTypes([col]);
    expect(res["created_at"]).toBe("DateTime");
  });

  it("Test column named price -> Price", () => {
    const col = column("product_price", "FLOAT");
    const res = mapColumnsToSemanticTypes([col]);
    expect(res["product_price"]).toBe("Price");
  });

  it("Test column named country -> Country", () => {
    const col = column("user_country", "VARCHAR");
    const res = mapColumnsToSemanticTypes([col]);
    expect(res["user_country"]).toBe("Country");
  });

  it("Test VARCHAR column with no name match -> Category", () => {
    const col = column("description", "VARCHAR");
    const res = mapColumnsToSemanticTypes([col]);
    expect(res["description"]).toBe("Category");
  });

  it("Test is_dttm flag overrides SQL type", () => {
    const col = column("date_value", "VARCHAR", true);
    const res = mapColumnsToSemanticTypes([col]);
    expect(res["date_value"]).toBe("DateTime");
  });

  it("Test multiple columns mapped correctly", () => {
    const columns: SupersetColumn[] = [
      column("id", "INT"),
      column("name", "VARCHAR"),
      column("price", "DECIMAL"),
      column("created", "TIMESTAMP", true),
    ];
    const res = mapColumnsToSemanticTypes(columns);
    expect(res["id"]).toBe("Quantity"); // fallback for INT
    expect(res["name"]).toBe("Name"); // name match
    expect(res["price"]).toBe("Price"); // name match
    expect(res["created"]).toBe("DateTime"); // is_dttm
  });
});
