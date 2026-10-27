// Where the Postgres control plane is (PG2 R9.1): `loams pg-control` serves
// loams.postgres.v1 on its own listener (127.0.0.1:7680 in the release compose
// stack, profile postgres-control), not on the engine's API port, and checks
// `authorization: Bearer lk_…` against its keys file. Main alone holds the key;
// the renderer reaches pg-control through loams-app://console/pg-control/.
// Pure: no electron import.
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import {
	DEFAULT_PG_CONTROL_URL,
	parsePgControlUrl,
} from "../../shared/pg-control";
import { readSetting } from "../settings";

export { DEFAULT_PG_CONTROL_URL } from "../../shared/pg-control";

/** settings.json: pg-control's base URL. */
export const PG_CONTROL_URL_KEY = "postgres.control.url";
/** settings.json: a file holding the API key (`lk_…`), as `pg-api-key` printed it. */
export const PG_CONTROL_KEY_FILE_KEY = "postgres.control.apiKeyFile";

/** Where the key main sends comes from; the key itself never leaves main. */
export type PgControlKeySource = "env" | "env-file" | "settings-file" | "none";

export interface PgControlTarget {
	/** The listener's origin, `http://127.0.0.1:7680`. */
	url: string;
	/** The API key, or null (a `--pg-authz dev-loopback` listener needs none). */
	apiKey: string | null;
}

/** settings.json's two pg-control values as saved; `""` when unset. */
export interface PgControlConfig {
	url: string;
	apiKeyFile: string;
}

/** A setting's or variable's URL as an origin, or null when it is not one. */
const httpUrl = (v: unknown): string | null => {
	if (typeof v !== "string" || v.trim() === "") return null;
	const p = parsePgControlUrl(v);
	if (p.ok) return p.url || null;
	// Older settings may carry a path; the origin is what was always used.
	try {
		const u = new URL(v.trim());
		return u.protocol === "http:" || u.protocol === "https:" ? u.origin : null;
	} catch {
		return null;
	}
};

/** `~/x` is the home directory's `x`; other paths are as given. */
export function expandHome(path: string): string {
	const p = path.trim();
	if (p === "~") return homedir();
	return p.startsWith("~/") ? `${homedir()}${p.slice(1)}` : p;
}

const readKeyFile = (path: unknown): string | null => {
	if (typeof path !== "string" || path.trim() === "") return null;
	try {
		return readFileSync(expandHome(path), "utf8").trim() || null;
	} catch {
		return null;
	}
};

const str = (v: unknown): string => (typeof v === "string" ? v.trim() : "");

/** settings.json's pg-control values. */
export function readPgControlConfig(settingsFile: string): PgControlConfig {
	return {
		url: str(readSetting<unknown>(settingsFile, PG_CONTROL_URL_KEY, "")),
		apiKeyFile: str(
			readSetting<unknown>(settingsFile, PG_CONTROL_KEY_FILE_KEY, ""),
		),
	};
}

/**
 * The URL from `LOAMS_PG_CONTROL_URL`, else the config's URL, else the release
 * stack's 127.0.0.1:7680. The key from `LOAMS_PG_CONTROL_API_KEY`, else the file
 * `LOAMS_PG_CONTROL_API_KEY_FILE` or the config's key file names.
 */
export function resolvePgControlTarget(
	config: PgControlConfig,
	env: NodeJS.ProcessEnv = process.env,
): PgControlTarget & { keySource: PgControlKeySource } {
	const url =
		httpUrl(env.LOAMS_PG_CONTROL_URL) ??
		httpUrl(config.url) ??
		DEFAULT_PG_CONTROL_URL;
	const fromEnv = env.LOAMS_PG_CONTROL_API_KEY?.trim();
	if (fromEnv) return { url, apiKey: fromEnv, keySource: "env" };
	const fromEnvFile = readKeyFile(env.LOAMS_PG_CONTROL_API_KEY_FILE);
	if (fromEnvFile) return { url, apiKey: fromEnvFile, keySource: "env-file" };
	const fromFile = readKeyFile(config.apiKeyFile);
	if (fromFile) return { url, apiKey: fromFile, keySource: "settings-file" };
	return { url, apiKey: null, keySource: "none" };
}

/**
 * The target for settings.json and the environment. Read per request, so a key
 * minted after the app started, or a change saved in Settings, is picked up.
 */
export function readPgControlTarget(
	settingsFile: string,
	env: NodeJS.ProcessEnv = process.env,
): PgControlTarget {
	const { url, apiKey } = resolvePgControlTarget(
		readPgControlConfig(settingsFile),
		env,
	);
	return { url, apiKey };
}
