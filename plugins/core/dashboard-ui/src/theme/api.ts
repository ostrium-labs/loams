import { API_BASE } from "../api";
import type { ThemePresetSummary, ThemeResolution } from "./types";

/**
 * Typed client for the theme endpoints on `apps/server`.
 *
 * Both endpoints are being built in parallel and may not exist when this ships,
 * so every failure mode here is a value the UI can render, never a throw that
 * escapes into the dashboard's load path. `ThemesUnavailableError` in
 * particular is what the picker degrades on: a dashboard that cannot load
 * because themes are unavailable would be a regression, so the theme catalog
 * failing must never gate `fetchDashboard`.
 */

export class ThemesUnavailableError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ThemesUnavailableError";
  }
}

const isPlainObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

function normalizeReport(raw: unknown): ThemeResolution["report"] {
  if (!Array.isArray(raw)) return [];
  return raw.filter(isPlainObject).map((entry) => ({
    stage: typeof entry.stage === "string" ? entry.stage : "unknown",
    path: typeof entry.path === "string" ? entry.path : "(root)",
    message: typeof entry.message === "string" ? entry.message : String(entry.message ?? ""),
  }));
}

/**
 * A 404 or a non-JSON body means the endpoint is not there yet. Both are
 * "unavailable", not "this theme is broken" — the picker says so rather than
 * showing an error the user can do nothing about.
 */
function isMissing(res: Response): boolean {
  return res.status === 404 || res.status === 501 || res.status === 405;
}

/** `GET /api/themes` — the catalogue, sorted by label. */
export async function fetchThemes(): Promise<ThemePresetSummary[]> {
  let res: Response;
  try {
    res = await fetch(`${API_BASE}/themes`);
  } catch (err) {
    throw new ThemesUnavailableError(
      `Theme service unreachable: ${err instanceof Error ? err.message : String(err)}`,
    );
  }

  if (isMissing(res)) {
    throw new ThemesUnavailableError(
      "The theme service is not available on this server. The dashboard is unthemed; charts still render.",
    );
  }
  if (!res.ok) {
    throw new ThemesUnavailableError(`Failed to load themes: ${res.statusText} (${res.status})`);
  }

  const body = await res.json().catch(() => null);
  const rows = isPlainObject(body) && Array.isArray(body.themes) ? body.themes : null;
  if (!rows) {
    throw new ThemesUnavailableError("The theme service returned an unexpected shape.");
  }

  return rows
    .filter(isPlainObject)
    .map((row) => ({
      id: typeof row.id === "string" ? row.id : "",
      label: typeof row.label === "string" ? row.label : String(row.id ?? "Unnamed theme"),
      description: typeof row.description === "string" ? row.description : "",
      icon: typeof row.icon === "string" ? row.icon : "",
    }))
    .filter((t) => t.id.length > 0);
}

/**
 * `GET /api/themes/:id` — the resolved spec plus flint's `report`.
 *
 * A theme that resolves but carries report entries is VALID and downgraded; a
 * theme that does not resolve comes back `valid: false` and must be shown as an
 * error, because flint treats an unknown house name as an error rather than
 * quietly rendering unthemed.
 */
export async function fetchTheme(id: string): Promise<ThemeResolution> {
  const res = await fetch(`${API_BASE}/themes/${encodeURIComponent(id)}`).catch(() => {
    throw new ThemesUnavailableError("The theme service is unreachable.");
  });

  if (isMissing(res)) {
    throw new ThemesUnavailableError(
      `The theme service cannot resolve "${id}" — the endpoint is not available on this server.`,
    );
  }
  if (!res.ok) {
    throw new ThemesUnavailableError(
      `Failed to resolve theme "${id}": ${res.statusText} (${res.status})`,
    );
  }

  const body = await res.json().catch(() => null);
  if (!isPlainObject(body)) {
    throw new ThemesUnavailableError(
      `The theme service returned an unreadable answer for "${id}".`,
    );
  }

  return {
    valid: body.valid === true,
    spec: isPlainObject(body.spec) ? body.spec : undefined,
    report: normalizeReport(body.report),
  };
}
