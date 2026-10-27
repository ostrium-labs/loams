// Log redaction (§25.6, LC2a global constraint "no plaintext address in logs").
// Tokens, addresses, bearer values and URL fragments are replaced before a line
// is written. Field values are redacted recursively.

import type { Logger } from "./ports.ts";

const PATTERNS: [RegExp, string][] = [
  [/\b(wlc|wlr)_[A-Za-z0-9_-]{8,}/g, "$1_[redacted]"],
  [/[^\s@<>()",;:'=&?#/]+@[a-z0-9-]+(\.[a-z0-9-]+)+/gi, "[email]"],
  [/(bearer\s+)[A-Za-z0-9._~+/-]+=*/gi, "$1[redacted]"],
  [/#[^\s"']*/g, "#[redacted]"],
  [/\b(token|turnstile_token|secret|code|password)(["']?\s*[:=]\s*["']?)[^\s"',}&]+/gi, "$1$2[redacted]"],
];

export function redact(s: string): string {
  let out = s;
  for (const [re, rep] of PATTERNS) out = out.replace(re, rep);
  return out;
}

function redactValue(v: unknown, depth = 0): unknown {
  if (typeof v === "string") return redact(v);
  if (depth > 4 || v === null || typeof v !== "object") return v;
  if (Array.isArray(v)) return v.map((x) => redactValue(x, depth + 1));
  const out: Record<string, unknown> = {};
  for (const [k, x] of Object.entries(v)) {
    out[k] = /token|secret|password|email|^to$|code/i.test(k) && typeof x === "string" ? "[redacted]" : redactValue(x, depth + 1);
  }
  return out;
}

export function redactFields(fields: Record<string, unknown>): Record<string, unknown> {
  return redactValue(fields) as Record<string, unknown>;
}

/** Wraps a sink so every line is redacted, whatever the caller passed. */
export function redactingLogger(sink: (line: string) => void): Logger {
  return {
    log(event, fields = {}) {
      sink(JSON.stringify({ event: redact(event), ...redactFields(fields) }));
    },
  };
}

/** console.log as one JSON line, redacted: Workers Logs picks it up. */
export const consoleLogger: Logger = redactingLogger((line) => console.log(line));
