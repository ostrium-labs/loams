import type { SupersetColumn } from "@loams-core/types";

export function mapColumnsToSemanticTypes(columns: SupersetColumn[]): Record<string, string> {
  const result: Record<string, string> = {};

  for (const col of columns) {
    const colName = (col.column_name || "").toLowerCase();
    const typeStr = (col.type || "").toUpperCase();

    let semanticType = "";

    // Type-based priority for explicit temporal types
    if (col.is_dttm || typeStr.includes("TIMESTAMP") || typeStr.includes("DATETIME")) {
      semanticType = "DateTime";
    } else if (
      colName.includes("price") ||
      colName.includes("cost") ||
      colName.includes("revenue")
    ) {
      semanticType = "Price";
    } else if (colName.includes("percent") || colName.includes("pct")) {
      semanticType = "Percentage";
    } else if (colName.includes("country")) {
      semanticType = "Country";
    } else if (
      colName.includes("quantity") ||
      colName.includes("qty") ||
      /(^|_)count(_|$)/.test(colName)
    ) {
      semanticType = "Quantity";
    } else if (colName.includes("rank")) {
      semanticType = "Rank";
    } else if (colName.includes("state")) {
      semanticType = "State";
    } else if (colName.includes("city")) {
      semanticType = "City";
    } else if (colName.includes("lat")) {
      semanticType = "Latitude";
    } else if (colName.includes("lon")) {
      semanticType = "Longitude";
    } else if (colName.includes("year")) {
      semanticType = "Year";
    } else if (colName.includes("month")) {
      semanticType = "Month";
    } else if (colName.includes("quarter")) {
      semanticType = "Quarter";
    } else if (
      colName.includes("date") ||
      colName.includes("created") ||
      colName.includes("updated") ||
      colName.endsWith("_at") ||
      colName.endsWith("_on")
    ) {
      semanticType = "Date";
    } else if (colName.includes("name")) {
      semanticType = "Name";
    } else if (colName.includes("status")) {
      semanticType = "Status";
    } else if (colName.includes("temperature") || colName.includes("temp")) {
      semanticType = "Temperature";
    }

    if (!semanticType) {
      if (col.is_dttm) {
        semanticType = "DateTime";
      } else if (typeStr.includes("DATE")) {
        semanticType = "Date";
      } else if (typeStr.includes("BIGINT") || typeStr.includes("INT")) {
        semanticType = "Quantity";
      } else if (
        typeStr.includes("FLOAT") ||
        typeStr.includes("DOUBLE") ||
        typeStr.includes("DECIMAL")
      ) {
        semanticType = "Amount";
      } else if (typeStr.includes("VARCHAR") || typeStr.includes("TEXT")) {
        semanticType = "Category";
      } else if (typeStr.includes("BOOLEAN") || typeStr.includes("BOOL")) {
        semanticType = "Boolean";
      } else {
        semanticType = "Category"; // Default fallback
      }
    }

    // `is_dttm` flag overrides to DateTime if it's explicitly set and the semantic type isn't inherently a date/time from the name.
    if (col.is_dttm) {
      semanticType = "DateTime";
    }

    result[col.column_name] = semanticType;
  }

  return result;
}
