import type { LocalShim } from "./handler.electron";

export interface LocalShimInfo {
	version: string;
	username: string;
}

const json = (status: number, body: unknown): Response =>
	new Response(JSON.stringify(body), {
		status,
		headers: { "content-type": "application/json" },
	});

/**
 * Answers the classic console's REST endpoints for the local engine, which has
 * no control plane. Shapes follow the apps mock (plan R0.9). Pure: no electron.
 * `kind` is consulted per request so a remote server is never shimmed.
 */
export function createLocalShim(
	info: LocalShimInfo,
	kind: () => string,
	now: () => number = Date.now,
): LocalShim {
	return (pathname, method) => {
		if (kind() !== "local") return null;
		if (!pathname.startsWith("/api/v1/")) return null;
		if (method === "GET" && pathname === "/api/v1/instance")
			return json(200, {
				name: "Loams",
				edition: "oss",
				version: info.version,
				setup_required: false,
				sign_in: { password: false, totp: false, passkeys: false, oidc: [] },
				features: {
					billing: false,
					multi_org: false,
					passkeys: false,
					local: true,
					desktop: true,
				},
			});
		if (method === "GET" && pathname === "/api/v1/session") {
			const t = now();
			return json(200, {
				user: {
					id: "local",
					name: info.username,
					email: null,
					avatar_url: null,
					two_factor: false,
					sso: null,
					created_at: new Date(t).toISOString().replace(/\.\d+Z$/, "Z"),
					last_seen_at: new Date(t).toISOString().replace(/\.\d+Z$/, "Z"),
				},
				org: {
					id: "local",
					name: "This computer",
					slug: "local",
					created_at: new Date(t).toISOString().replace(/\.\d+Z$/, "Z"),
					require_two_factor: false,
					allowed_domains: [],
				},
				role: "owner",
				csrf_token: "local",
				expires_at: new Date(t + 12 * 3600_000)
					.toISOString()
					.replace(/\.\d+Z$/, "Z"),
			});
		}
		return json(404, {
			code: "not_in_local_edition",
			message: "This needs a Loams control plane. Add a server in Servers.",
		});
	};
}
