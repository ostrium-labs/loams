import { readFileSync } from "node:fs";
import { create, fromBinary, toBinary, toJson } from "@bufbuild/protobuf";
import { auth, instance } from "@loams/proto";
import { describe, expect, it } from "vite-plus/test";
import { createLocalShim } from "../src/main/protocol/local-shim";

const fixture = (n: string) =>
	JSON.parse(
		readFileSync(new URL(`./fixtures/${n}`, import.meta.url), "utf8"),
	) as unknown;

/** Key tree: object keys recursively; leaves are ignored (values differ locally). */
function shape(v: unknown): unknown {
	if (v && typeof v === "object" && !Array.isArray(v))
		return Object.fromEntries(
			Object.entries(v)
				.sort(([a], [b]) => a.localeCompare(b))
				.map(([k, x]) => [k, shape(x)]),
		);
	return "leaf";
}

const shim = createLocalShim(
	{ version: "1.2.3", username: "dina" },
	() => "local",
	() => Date.parse("2026-10-13T09:00:00Z"),
);

const post = (path: string, type = "application/json", body = "{}") =>
	new Request(`loams-app://console${path}`, {
		method: "POST",
		headers: { "content-type": type },
		body,
	});

const unreachable = () => Promise.reject(new Error("must not forward"));

/** The engine's GetInstance answer, as the proxy would return it. */
const engine = (type: "application/json" | "application/proto") => () => {
	const answer = create(instance.GetInstanceResponseSchema, {
		name: "Loams",
		serverVersion: "1.2.3",
		apiVersions: ["loams.instance.v1", "loams.graph.v1"],
		features: { live: true },
		// A fresh engine with authentication off: its setup gate is open.
		setupRequired: true,
	});
	const body =
		type === "application/proto"
			? toBinary(instance.GetInstanceResponseSchema, answer)
			: JSON.stringify(toJson(instance.GetInstanceResponseSchema, answer));
	return Promise.resolve(
		new Response(body, { headers: { "content-type": type } }),
	);
};

describe("local shim", () => {
	it("desktop_local_shim_answers_connect", async () => {
		// GetSession: the person at this computer, shaped as the mock answers it.
		const session = shim(
			"/loams.auth.v1.AuthService/GetSession",
			post("/loams.auth.v1.AuthService/GetSession"),
			unreachable,
		);
		const res = await session;
		expect(res?.status).toBe(200);
		const body = (await res?.json()) as Record<string, unknown>;
		const mock = fixture("apps-mock-get-session.json") as {
			session: { user: object; org: object };
		};
		expect(Object.keys(shape(body) as object)).toEqual(
			Object.keys(shape(mock) as object),
		);
		expect(body).toMatchObject({
			session: {
				user: { id: "local", name: "dina" },
				org: { id: "local", name: "This computer", slug: "local" },
				role: "ORG_ROLE_OWNER",
				csrfToken: "local",
				expiresAt: "2026-10-13T21:00:00Z",
			},
		});

		// The same over the binary codec the console's transport uses.
		const bin = await shim(
			"/loams.auth.v1.AuthService/GetSession",
			post("/loams.auth.v1.AuthService/GetSession", "application/proto", ""),
			unreachable,
		);
		expect(bin?.headers.get("content-type")).toBe("application/proto");
		const decoded = fromBinary(
			auth.GetSessionResponseSchema,
			new Uint8Array((await bin?.arrayBuffer()) ?? new ArrayBuffer(0)),
		);
		expect(decoded.session?.user?.name).toBe("dina");

		// GetInstance: the engine's answer, with the desktop's features and loams.auth.v1.
		for (const type of ["application/json", "application/proto"] as const) {
			const path = "/loams.instance.v1.InstanceService/GetInstance";
			const r = await shim(path, post(path, type), engine(type));
			const answer =
				type === "application/proto"
					? fromBinary(
							instance.GetInstanceResponseSchema,
							new Uint8Array((await r?.arrayBuffer()) ?? new ArrayBuffer(0)),
						)
					: ((await r?.json()) as { apiVersions: string[] });
			expect(answer.apiVersions).toEqual([
				"loams.instance.v1",
				"loams.graph.v1",
				"loams.auth.v1",
			]);
			if (type === "application/proto") {
				expect((answer as instance.GetInstanceResponse).features).toEqual({
					live: true,
					local: true,
					desktop: true,
				});
				expect((answer as instance.GetInstanceResponse).setupRequired).toBe(
					false,
				);
			} else {
				expect(
					(answer as { setupRequired?: boolean }).setupRequired ?? false,
				).toBe(false);
			}
		}
	});

	it("other_console_rpcs_need_a_control_plane", async () => {
		for (const path of [
			"/loams.auth.v1.AuthService/CreateSession",
			"/loams.admin.v1.ProjectService/ListProjects",
		]) {
			const r = await shim(path, post(path), unreachable);
			expect(r?.status).toBe(501);
			expect(await r?.json()).toEqual({
				code: "unimplemented",
				message: "This needs a Loams control plane. Add a server in Servers.",
			});
		}
		const providers = await shim(
			"/loams.auth.v1.AuthService/ListProviders",
			post("/loams.auth.v1.AuthService/ListProviders"),
			unreachable,
		);
		expect(await providers?.json()).toEqual({});
	});

	it("other_paths_are_not_the_shims", () => {
		for (const path of [
			"/loams.approvals.v1.ApprovalService/ListApprovals",
			"/api/v1/instance",
			"/v1/namespaces/default/sql",
			"/",
		])
			expect(shim(path, post(path), unreachable)).toBe(null);
	});

	it("shim_inactive_for_remote", () => {
		const remote = createLocalShim(
			{ version: "1", username: "u" },
			() => "remote",
		);
		const path = "/loams.auth.v1.AuthService/GetSession";
		expect(remote(path, post(path), unreachable)).toBe(null);
	});
});
