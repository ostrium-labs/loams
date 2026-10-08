// The servers.* IPC handlers without Electron, so argument checks and the
// activate sequence (I4: cookies cleared before the reload) are unit-tested.
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
	get(filter: {
		url: string;
	}): Promise<ReadonlyArray<{ name: string; path?: string }>>;
	remove(url: string, name: string): Promise<void>;
}

/** Drops every cookie of the loams-app://console origin (they belong to the previous server). */
export async function clearConsoleCookies(jar: CookieJarLike): Promise<void> {
	const cookies = await jar.get({ url: `${APP_ORIGIN}/` });
	for (const c of cookies)
		await jar.remove(`${APP_ORIGIN}${c.path || "/"}`, c.name);
}

export interface ServerHandlerDeps {
	clearCookies: () => Promise<void>;
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
			const r = registry.activate(id);
			if (!r.ok) return r;
			// Session cookies of the old server must not ride along to the new one.
			await d.clearCookies().catch(() => undefined);
			d.reload();
			return r;
		},
	};
}
