import { readFile, stat } from "node:fs/promises";
import { protocol, type Session } from "electron";
import type { ServerEntry } from "../../shared/contracts";
import { isProxied, proxyRequest } from "./proxy";
import { resolveStatic } from "./static";

export const APP_SCHEME = "loams-app";

export type LocalShim = (pathname: string, method: string) => Response | null;

export interface AppProtocolDeps {
	distRoot: string;
	activeServer: () => ServerEntry;
	/** Answers a few endpoints itself when the active server is the local engine. */
	localShim: LocalShim;
}

/** Must run before app ready. */
export function registerAppScheme(): void {
	protocol.registerSchemesAsPrivileged([
		{
			scheme: APP_SCHEME,
			privileges: {
				standard: true,
				secure: true,
				supportFetchAPI: true,
				corsEnabled: false,
				stream: true,
			},
		},
	]);
}

const text = (status: number, body: string) =>
	new Response(body, {
		status,
		headers: { "content-type": "text/plain; charset=utf-8" },
	});

async function serveFile(file: string, mime: string): Promise<Response> {
	try {
		if (!(await stat(file)).isFile()) return text(404, "not found");
		return new Response(new Uint8Array(await readFile(file)), {
			headers: { "content-type": mime, "cache-control": "no-cache" },
		});
	} catch {
		return text(404, "not found");
	}
}

export function installAppProtocol(ses: Session, deps: AppProtocolDeps): void {
	ses.protocol.handle(APP_SCHEME, async (req) => {
		const url = new URL(req.url);
		if (url.hostname !== "console") return text(404, "not found");

		if (isProxied(url.pathname)) {
			const server = deps.activeServer();
			if (server.kind === "local" && server.url === "")
				return new Response(
					JSON.stringify({
						code: "engine_not_ready",
						message: "the local engine is not ready yet",
					}),
					{ status: 503, headers: { "content-type": "application/json" } },
				);
			if (server.kind === "local") {
				const shimmed = deps.localShim(url.pathname, req.method);
				if (shimmed) return shimmed;
			}
			try {
				return await proxyRequest(req, server.url, (r) =>
					ses.fetch(r as Request),
				);
			} catch (e) {
				return new Response(
					JSON.stringify({
						code: "unreachable",
						message: `cannot reach ${server.name}: ${(e as Error).message}`,
					}),
					{ status: 502, headers: { "content-type": "application/json" } },
				);
			}
		}

		const r = resolveStatic(deps.distRoot, url);
		if (!r) return text(404, "not found");
		if ("spaFallback" in r)
			return serveFile(r.spaFallback, "text/html; charset=utf-8");
		return serveFile(r.file, r.mime);
	});
}
