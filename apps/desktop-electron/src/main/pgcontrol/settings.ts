// App settings > Postgres control plane (PG2 R9.1): checks, saves and tests the
// pg-control URL and the API key file. The key is read here, in main, and only
// ever goes into the Authorization header of a request to pg-control: never into
// a result, a message or a log. Pure: no electron import.
import { readFileSync, statSync } from "node:fs";
import { isAbsolute } from "node:path";
import type {
	PgControlFieldErrors,
	PgControlSaveResult,
	PgControlSettings,
	PgControlSettingsInput,
	PgControlTestResult,
} from "../../shared/contracts";
import {
	DEFAULT_PG_CONTROL_URL,
	parsePgControlUrl,
} from "../../shared/pg-control";
import { writeSetting } from "../settings";
import {
	expandHome,
	PG_CONTROL_KEY_FILE_KEY,
	PG_CONTROL_URL_KEY,
	readPgControlConfig,
	resolvePgControlTarget,
} from "./target";

/** A key file is one line, a few dozen bytes; anything much larger is the wrong file. */
const MAX_KEY_FILE_BYTES = 4096;

const ENV_OVERRIDES = [
	"LOAMS_PG_CONTROL_URL",
	"LOAMS_PG_CONTROL_API_KEY",
	"LOAMS_PG_CONTROL_API_KEY_FILE",
] as const;

/** What the Settings page shows: the saved values and what wins over them. */
export function pgControlView(
	settingsFile: string,
	env: NodeJS.ProcessEnv = process.env,
): PgControlSettings {
	const config = readPgControlConfig(settingsFile);
	const target = resolvePgControlTarget(config, env);
	const parsed = parsePgControlUrl(config.url);
	return {
		url: parsed.ok ? parsed.url : config.url,
		apiKeyFile: config.apiKeyFile,
		defaultUrl: DEFAULT_PG_CONTROL_URL,
		effectiveUrl: target.url,
		envOverrides: ENV_OVERRIDES.filter((k) => (env[k] ?? "").trim() !== ""),
		keySource: target.keySource,
	};
}

/** The key in `path`, or why it cannot be used. */
function readKey(path: string): { key: string } | { error: string } {
	const p = expandHome(path);
	if (!isAbsolute(p))
		return { error: "Use an absolute path, like ~/.config/loams/pg-api-key." };
	let size: number;
	try {
		const st = statSync(p);
		if (!st.isFile()) return { error: "This is not a file." };
		size = st.size;
	} catch {
		return { error: "No file at this path." };
	}
	if (size > MAX_KEY_FILE_BYTES)
		return { error: "This file is too large to be an API key file." };
	let text: string;
	try {
		text = readFileSync(p, "utf8").trim();
	} catch {
		return { error: "The app cannot read this file." };
	}
	if (text === "") return { error: "The file is empty." };
	if (/\s/.test(text))
		return {
			error: "The file must hold one API key, as pg-api-key printed it.",
		};
	return { key: text };
}

export interface CheckedInput {
	value: PgControlSettingsInput;
	errors: PgControlFieldErrors;
	/** The key in the file, when one is named and readable. Main only. */
	key: string | null;
}

/** Normalizes the form's values and checks them; the URL as an origin, the path trimmed. */
export function checkPgControlInput(input: unknown): CheckedInput {
	const errors: PgControlFieldErrors = {};
	const raw =
		input && typeof input === "object"
			? (input as Record<string, unknown>)
			: {};
	const url = typeof raw.url === "string" ? raw.url : "";
	const apiKeyFile =
		typeof raw.apiKeyFile === "string" ? raw.apiKeyFile.trim() : "";
	if (url.length > 2048) errors.url = "This URL is too long.";
	const parsed = parsePgControlUrl(url.slice(0, 2048));
	if (!parsed.ok) errors.url = parsed.message;
	let key: string | null = null;
	if (apiKeyFile.length > 4096) errors.apiKeyFile = "This path is too long.";
	else if (apiKeyFile !== "") {
		const r = readKey(apiKeyFile);
		if ("error" in r) errors.apiKeyFile = r.error;
		else key = r.key;
	}
	return {
		value: { url: parsed.ok ? parsed.url : url.trim(), apiKeyFile },
		errors,
		key,
	};
}

const hasErrors = (e: PgControlFieldErrors) => Object.keys(e).length > 0;

/** Saves both values (an empty one removes the setting), or says which field is wrong. */
export function savePgControlSettings(
	settingsFile: string,
	input: unknown,
	env: NodeJS.ProcessEnv = process.env,
): PgControlSaveResult {
	const { value, errors } = checkPgControlInput(input);
	if (hasErrors(errors))
		return {
			ok: false,
			code: "invalid_argument",
			message: "Some values need fixing.",
			errors,
		};
	try {
		// `undefined` drops the key from settings.json: the default applies again.
		writeSetting(settingsFile, PG_CONTROL_URL_KEY, value.url || undefined);
		writeSetting(
			settingsFile,
			PG_CONTROL_KEY_FILE_KEY,
			value.apiKeyFile || undefined,
		);
	} catch (e) {
		return { ok: false, code: "io_error", message: (e as Error).message };
	}
	return { ok: true, value: pgControlView(settingsFile, env) };
}

const scrub = (text: string, key: string | null): string =>
	key ? text.split(key).join("[redacted]") : text;

/**
 * "Test connection": one `ListOperations` call with the form's values (not yet
 * saved), resolved the way requests will be once they are saved, so an
 * environment variable that wins over them is what is tested.
 */
export async function testPgControl(
	input: unknown,
	fetchImpl: typeof fetch,
	env: NodeJS.ProcessEnv = process.env,
	timeoutMs = 5000,
): Promise<PgControlTestResult> {
	const checked = checkPgControlInput(input);
	if (hasErrors(checked.errors))
		return {
			ok: false,
			code: "invalid",
			message: "Fix the highlighted values first.",
			url: checked.value.url || DEFAULT_PG_CONTROL_URL,
			errors: checked.errors,
		};
	const target = resolvePgControlTarget(checked.value, env);
	const key = target.apiKey;
	const started = Date.now();
	let res: Response;
	try {
		res = await fetchImpl(
			`${target.url}/loams.operations.v1.OperationsService/ListOperations`,
			{
				method: "POST",
				headers: {
					"content-type": "application/json",
					"connect-protocol-version": "1",
					...(key ? { authorization: `Bearer ${key}` } : {}),
				},
				body: JSON.stringify({ namespace: "default", pageSize: 1 }),
				redirect: "manual",
				signal: AbortSignal.timeout(timeoutMs),
			},
		);
	} catch (e) {
		const err = e as Error;
		const timedOut = err.name === "TimeoutError" || err.name === "AbortError";
		return {
			ok: false,
			code: "unreachable",
			message: timedOut
				? `No answer from ${target.url} within ${Math.round(timeoutMs / 1000)} s.`
				: scrub(
						`Nothing answered at ${target.url}: ${(err.cause as Error | undefined)?.message ?? err.message}`,
						key,
					),
			url: target.url,
		};
	}
	const ms = Date.now() - started;
	const body = (await res.json().catch(() => ({}))) as {
		code?: unknown;
		message?: unknown;
	};
	const detail =
		typeof body.message === "string" ? scrub(body.message, key) : "";
	const at = { url: target.url, ms };
	if (res.ok)
		return {
			ok: true,
			code: "ok",
			message:
				target.keySource === "none"
					? `pg-control answered in ${ms} ms, without an API key.`
					: `pg-control answered in ${ms} ms and accepted the API key.`,
			...at,
		};
	if (res.status === 401 || body.code === "unauthenticated")
		return {
			ok: false,
			code: "unauthenticated",
			message: key
				? "pg-control refused the API key. Mint a new one with pg-api-key."
				: "pg-control needs an API key. Choose the key file.",
			...at,
		};
	if (res.status === 403 || body.code === "permission_denied")
		return {
			ok: false,
			code: "permission_denied",
			message: `pg-control accepted the key but refused the call${detail ? `: ${detail}` : "."}`,
			...at,
		};
	if (res.status === 404 || body.code === "unimplemented")
		return {
			ok: false,
			code: "not_pg_control",
			message: `Something answered at ${target.url}, but not pg-control's API (HTTP ${res.status}).`,
			...at,
		};
	return {
		ok: false,
		code: "error",
		message: `pg-control answered HTTP ${res.status}${detail ? `: ${detail}` : "."}`,
		...at,
	};
}
