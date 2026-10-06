/**
 * Typed fetch wrappers for the plugin control plane.
 *
 * The only interesting part of this file is `request`: it exists so that every
 * page in the shell reports the *same* useful message on failure. Without it
 * each caller writes its own `res.statusText` fallback and the console ends up
 * saying "Failed to fetch" for three different underlying problems.
 */
import type { PluginAgentSkill, PluginManifest, PluginStatus } from "../types.js";

/** The same base `dashboard-ui` uses; kept here so core's UI is self-contained. */
export const API_BASE = "/api";

/** An A2A AgentCard as served by `GET /api/plugins/:id/agent`. */
export interface AgentCard {
  name: string;
  description: string;
  version: string;
  url?: string;
  skills?: PluginAgentSkill[];
  [key: string]: unknown;
}

export class ApiError extends Error {
  readonly status: number;
  readonly url: string;

  constructor(message: string, status: number, url: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.url = url;
  }

  /** True when we never got an HTTP response at all. */
  get isNetworkError(): boolean {
    return this.status === 0;
  }
}

function readMessage(body: unknown): string | null {
  if (!body || typeof body !== "object") return null;
  const record = body as Record<string, unknown>;
  if (typeof record.error === "string" && record.error.length > 0) return record.error;
  if (typeof record.message === "string" && record.message.length > 0) return record.message;
  return null;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const url = `${API_BASE}${path}`;

  let response: Response;
  try {
    response = await fetch(url, init);
  } catch {
    // A rejected fetch is nearly always "the API is not running", and saying
    // "Failed to fetch" hides that from a user who just started `vp dev`.
    throw new ApiError(
      `Could not reach the server at ${API_BASE}. Check that it is running.`,
      0,
      url,
    );
  }

  const raw = await response.text().catch(() => "");
  let body: unknown;
  let parsed = false;
  if (raw.length > 0) {
    try {
      body = JSON.parse(raw);
      parsed = true;
    } catch {
      body = undefined;
    }
  }

  if (!response.ok) {
    const serverMessage = readMessage(body);
    if (serverMessage) throw new ApiError(serverMessage, response.status, url);
    if (!parsed) {
      throw new ApiError(
        `${response.status} ${response.statusText || "Request failed"} (response was not JSON)`,
        response.status,
        url,
      );
    }
    throw new ApiError(
      `${response.status} ${response.statusText || "Request failed"} for ${path}`,
      response.status,
      url,
    );
  }

  if (!parsed) {
    throw new ApiError(
      "The server returned an empty response where data was expected.",
      response.status,
      url,
    );
  }

  return body as T;
}

/**
 * `GET /api/plugins` -> `{ plugins: PluginStatus[] }`
 *
 * The server already sorts the list (always-on first, then `order`, then name)
 * and every page here renders it in the order it arrives, so the console and
 * the plugins page agree without duplicating the sort rule client-side.
 */
export async function fetchPlugins(): Promise<PluginStatus[]> {
  const body = await request<{ plugins?: PluginStatus[] } | PluginStatus[]>("/plugins");
  const list = Array.isArray(body) ? body : body.plugins;
  return Array.isArray(list) ? list : [];
}

export async function fetchPlugin(id: string): Promise<PluginStatus> {
  return request<PluginStatus>(`/plugins/${encodeURIComponent(id)}`);
}

/**
 * `POST /api/plugins/:id/enable` / `.../disable` -> the updated `PluginStatus`.
 *
 * The returned status is the authority; callers must not assume the request
 * simply succeeded because it did not throw.
 */
export async function setPluginEnabled(id: string, enabled: boolean): Promise<PluginStatus> {
  const verb = enabled ? "enable" : "disable";
  return request<PluginStatus>(`/plugins/${encodeURIComponent(id)}/${verb}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
  });
}

export async function fetchPluginAgentCard(id: string): Promise<AgentCard> {
  return request<AgentCard>(`/plugins/${encodeURIComponent(id)}/agent`);
}

export type { PluginAgentSkill, PluginManifest, PluginStatus };
