// HTTP conventions for /cloud/v1 (design 78 §25.10, first draft's conventions):
// JSON; errors as {"error":{"code","message","request_id"}}; every response
// carries request_id; public endpoints send no-store and no-referrer; request
// bodies are at most 8 KiB.

export const MAX_BODY = 8 * 1024;

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly headers: Record<string, string>;
  constructor(status: number, code: string, message: string, headers: Record<string, string> = {}) {
    super(message);
    this.status = status;
    this.code = code;
    this.headers = headers;
  }
}

export const invalid = (message: string) => new ApiError(400, "INVALID_REQUEST", message);
export const rateLimited = (retryAfterS: number) =>
  new ApiError(429, "RATE_LIMITED", "too many requests; try again later", { "retry-after": String(Math.max(1, Math.ceil(retryAfterS))) });

function baseHeaders(requestId: string): Record<string, string> {
  return {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "referrer-policy": "no-referrer",
    "x-content-type-options": "nosniff",
    "x-request-id": requestId,
  };
}

export function json(requestId: string, status: number, body: unknown, headers: Record<string, string> = {}): Response {
  if (status === 204) return new Response(null, { status, headers: { ...baseHeaders(requestId), ...headers } });
  const payload = body && typeof body === "object" && !Array.isArray(body) ? { ...(body as object), request_id: requestId } : body;
  return new Response(JSON.stringify(payload), { status, headers: { ...baseHeaders(requestId), ...headers } });
}

export function errorResponse(requestId: string, e: ApiError): Response {
  return new Response(JSON.stringify({ error: { code: e.code, message: e.message, request_id: requestId } }), {
    status: e.status,
    headers: { ...baseHeaders(requestId), ...e.headers },
  });
}

/** Reads a JSON object body of at most 8 KiB. */
export async function readJson(req: Request): Promise<Record<string, unknown>> {
  const len = Number(req.headers.get("content-length") ?? "0");
  if (len > MAX_BODY) throw new ApiError(413, "INVALID_REQUEST", "the request body is larger than 8 KiB");
  const text = await req.text();
  if (new TextEncoder().encode(text).length > MAX_BODY) throw new ApiError(413, "INVALID_REQUEST", "the request body is larger than 8 KiB");
  let v: unknown;
  try {
    v = text ? JSON.parse(text) : {};
  } catch {
    throw invalid("the body is not valid JSON");
  }
  if (!v || typeof v !== "object" || Array.isArray(v)) throw invalid("the body must be a JSON object");
  return v as Record<string, unknown>;
}

export function optString(body: Record<string, unknown>, key: string, max: number): string | undefined {
  const v = body[key];
  if (v === undefined || v === null || v === "") return undefined;
  if (typeof v !== "string") throw invalid(`${key} must be a string`);
  if (v.length > max) throw invalid(`${key} is longer than ${max} characters`);
  return v;
}

export function reqString(body: Record<string, unknown>, key: string, max: number): string {
  const v = optString(body, key, max);
  if (v === undefined) throw invalid(`${key} is required`);
  return v;
}

/** The client address Cloudflare saw. */
export function clientIp(req: Request): string {
  return req.headers.get("cf-connecting-ip") ?? "unknown";
}
