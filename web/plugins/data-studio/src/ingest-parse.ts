// Parses an ingest file in the renderer: a `.json` array, or `.ndjson` with
// one object per line. A bad line reports its 1-based number.

import type { JsonObject } from '@bufbuild/protobuf';

export class ParseError extends Error {
  constructor(
    message: string,
    readonly line?: number,
  ) {
    super(message);
    this.name = 'ParseError';
  }
}

const isObject = (v: unknown): v is JsonObject =>
  typeof v === 'object' && v !== null && !Array.isArray(v);

export function parseNdjson(text: string): JsonObject[] {
  const out: JsonObject[] = [];
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i] ?? '';
    if (raw.trim() === '') continue;
    let value: unknown;
    try {
      value = JSON.parse(raw);
    } catch (e) {
      throw new ParseError(
        `Line ${i + 1} is not valid JSON: ${e instanceof Error ? e.message : String(e)}`,
        i + 1,
      );
    }
    if (!isObject(value)) throw new ParseError(`Line ${i + 1} is not a JSON object.`, i + 1);
    out.push(value);
  }
  return out;
}

export function parseJsonArray(text: string): JsonObject[] {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (e) {
    throw new ParseError(
      `The file is not valid JSON: ${e instanceof Error ? e.message : String(e)}`,
    );
  }
  if (!Array.isArray(value)) throw new ParseError('A .json file must hold an array of objects.');
  value.forEach((v, i) => {
    if (!isObject(v)) throw new ParseError(`Item ${i + 1} is not a JSON object.`);
  });
  return value as JsonObject[];
}

export function parseIngest(filename: string, text: string): JsonObject[] {
  return /\.(ndjson|jsonl)$/i.test(filename) ? parseNdjson(text) : parseJsonArray(text);
}

export function chunk<T>(items: T[], size: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
  return out;
}
