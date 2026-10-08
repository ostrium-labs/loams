// The servers.* IPC handlers without Electron, so argument checks and the
// activate sequence (I4: the departing session cleared before the reload) are unit-tested.
import type { IpcResult, ServerEntry } from "../../shared/contracts";
import { APP_ORIGIN } from "../security/policy";
import type { ServerRegistry } from "./registry";

const bad = (message: string): IpcResult<never> => ({
	ok: false,
	code: "bad_request",
	message,
});

const MAX_NAME = 200;
const MAX_URL = 2048;
const MAX_ID = 128;

const isId = (v: unknown): v is string =>
	typeof v === "string" && v.length > 0 && v.length <= MAX_ID;

export interface CookieJarLike {
	get(filter: Record<string, never>): Promise<
		ReadonlyArray<{
			name: string;
			domain?: string;
			path?: string;
			secure?: boolean;
		}>
	>;
	remove(url: string, name: string): Promise<void>;
}

const CONSOLE_HOST = new URL(APP_ORIGIN).hostname;

/** True when a cookie for `domain` is sent to `host` (host-only, or a parent `.domain`). */
function reaches(domain: string, host: string): boolean {
	if (!domain.startsWith(".")) return domain === host;
	return host === domain.slice(1) || host.endsWith(domain);
}

/**
 * Drops the session of the server being left. Its cookies are not stored under
 * loams-app://console (Chromium keeps no cookies for that scheme): the proxy's
 * `session.fetch` stores them under the server's own host, and cookies ignore the
 * port, so 127.0.0.1:8084's session would otherwise reach 127.0.0.1:<engine>.
 * Any console cookie goes too. The jar is listed unfiltered and matched on domain: a
 * `url` filter is path-matched and would miss a `Path=/api` cookie.
 */
export async function clearSessionCookies(
	jar: CookieJarLike,
	origin: string,
): Promise<void> {
	let server: URL | undefined;
	try {
		server = origin ? new URL(origin) : undefined;
	} catch {
		server = undefined;
	}
	for (const c of await jar.get({})) {
		const domain = c.domain ?? "";
		const bare = domain.replace(/^\./, "");
		const path = c.path || "/";
		if (bare === CONSOLE_HOST) await jar.remove(`${APP_ORIGIN}${path}`, c.name);
		else if (server && reaches(domain, server.hostname)) {
			const scheme = c.secure ? "https:" : server.protocol;
			await jar.remove(`${scheme}//${bare}${path}`, c.name);
		}
	}
}

export interface ServerHandlerDeps {
	/** Clears the session of the server at `origin` (the one being left). */
	clearCookies: (origin: string) => Promise<void>;
	reload: () => void;
}

export function serverHandlers(registry: ServerRegistry, d: ServerHandlerDeps) {
	return {
		list: () => registry.list(),
		add: async (raw: unknown): Promise<IpcResult<ServerEntry>> => {
			if (typeof raw !== "object" || raw === null || Array.isArray(raw))
				return bad("Invalid server");
			const { name, url } = raw as Record<string, unknown>;
			if (typeof name !== "string" || name.length > MAX_NAME)
				return bad("Invalid server name");
			if (typeof url !== "string" || url.length > MAX_URL)
				return bad("Invalid server URL");
			return registry.add({ name, url, kind: "remote" });
		},
		remove: async (id: unknown): Promise<IpcResult<void>> =>
			isId(id) ? registry.remove(id) : bad("Invalid server id"),
		activate: async (id: unknown): Promise<IpcResult<void>> => {
			if (!isId(id)) return bad("Invalid server id");
			const leaving = registry.active();
			const r = registry.activate(id);
			if (!r.ok) return r;
			// The old server's session must not ride along to the new one.
			if (leaving.id !== id)
				await d.clearCookies(leaving.url).catch(() => undefined);
			d.reload();
			return r;
		},
	};
}
