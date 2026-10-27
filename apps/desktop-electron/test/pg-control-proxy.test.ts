// PG2 R9.1: loams-app://console/pg-control/* goes to pg-control's own listener
// (not the active server), with the API key only main holds; only
// loams.postgres.v1 and pg-control's operations reads pass.
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import {
	handlePgControlRequest,
	PG_CONTROL_STATUS_HEADER,
	pgControlUpstreamPath,
} from "../src/main/protocol/pg-control";
import {
	DEFAULT_PG_CONTROL_URL,
	readPgControlTarget,
} from "../src/main/pgcontrol/target";

const KEY = "lk_01K7AAAAAAAAAAAAAAAAAAAAAA_abcdefghijklmnopqrstuvwxyz012345";
const LIST = "/pg-control/loams.postgres.v1.PostgresService/ListProjects";

const rendererRequest = (path: string) =>
	new Request(`loams-app://console${path}`, {
		method: "POST",
		headers: {
			"content-type": "application/json",
			origin: "loams-app://console",
			cookie: "session=renderer",
			authorization: "Bearer renderer-guess",
		},
		body: '{"namespace":"default"}',
	});

describe("pgControlUpstreamPath", () => {
	it("strips the prefix for pg-control's methods only", () => {
		expect(pgControlUpstreamPath(LIST)).toBe(
			"/loams.postgres.v1.PostgresService/ListProjects",
		);
		expect(
			pgControlUpstreamPath(
				"/pg-control/loams.operations.v1.OperationsService/ListOperations",
			),
		).toBe("/loams.operations.v1.OperationsService/ListOperations");
		for (const p of [
			"/pg-control/loams.operations.v1.OperationsService/CancelOperation",
			"/pg-control/loams.instance.v1.InstanceService/GetInstance",
			"/pg-control/loams.postgres.v1.PostgresService/../x",
			"/loams.postgres.v1.PostgresService/ListProjects",
		])
			expect(pgControlUpstreamPath(p)).toBeNull();
	});
});

describe("handlePgControlRequest", () => {
	it("forwards to pg-control with main's key and none of the renderer's credentials", async () => {
		let seen: Request | undefined;
		const res = await handlePgControlRequest(
			rendererRequest(LIST),
			() => ({ url: "http://127.0.0.1:7680", apiKey: KEY }),
			async (r) => {
				seen = r as Request;
				return new Response('{"projects":[]}', {
					headers: { "content-type": "application/json", "x-echo": KEY },
				});
			},
		);
		expect(res.status).toBe(200);
		expect(seen?.url).toBe(
			"http://127.0.0.1:7680/loams.postgres.v1.PostgresService/ListProjects",
		);
		expect(seen?.headers.get("authorization")).toBe(`Bearer ${KEY}`);
		expect(seen?.headers.get("cookie")).toBeNull();
		expect(seen?.headers.get("origin")).toBeNull();
		// The key never reaches the renderer, even echoed.
		expect(res.headers.get("x-echo")).toBeNull();
		expect(await res.text()).toBe('{"projects":[]}');
	});

	it("sends no authorization without a key (dev-loopback)", async () => {
		let seen: Request | undefined;
		await handlePgControlRequest(
			rendererRequest(LIST),
			() => ({ url: "http://127.0.0.1:7680", apiKey: null }),
			async (r) => {
				seen = r as Request;
				return new Response("{}");
			},
		);
		expect(seen?.headers.get("authorization")).toBeNull();
	});

	it("answers 503, marked, when pg-control is not there", async () => {
		const res = await handlePgControlRequest(
			rendererRequest(LIST),
			() => ({ url: "http://127.0.0.1:7680", apiKey: KEY }),
			async () => {
				throw new TypeError("fetch failed");
			},
		);
		expect(res.status).toBe(503);
		expect(res.headers.get(PG_CONTROL_STATUS_HEADER)).toBe("unreachable");
		expect(await res.json()).toMatchObject({ code: "unavailable" });
	});

	it("refuses other paths", async () => {
		let called = false;
		const res = await handlePgControlRequest(
			rendererRequest("/pg-control/loams.admin.v1.KeyService/ListKeys"),
			() => ({ url: "http://127.0.0.1:7680", apiKey: KEY }),
			async () => {
				called = true;
				return new Response("{}");
			},
		);
		expect(res.status).toBe(404);
		expect(called).toBe(false);
	});
});

describe("readPgControlTarget", () => {
	const dir = mkdtempSync(join(tmpdir(), "pgc-"));
	const settings = join(dir, "settings.json");

	it("defaults to the release stack's listener and no key", () => {
		expect(readPgControlTarget(join(dir, "missing.json"), {})).toEqual({
			url: DEFAULT_PG_CONTROL_URL,
			apiKey: null,
		});
	});

	it("reads the URL and key file from settings.json", () => {
		const keyFile = join(dir, "pg-api-key");
		writeFileSync(keyFile, `${KEY}\n`);
		writeFileSync(
			settings,
			JSON.stringify({
				"postgres.control.url": "http://10.0.0.5:7680/",
				"postgres.control.apiKeyFile": keyFile,
			}),
		);
		expect(readPgControlTarget(settings, {})).toEqual({
			url: "http://10.0.0.5:7680",
			apiKey: KEY,
		});
	});

	it("prefers the environment", () => {
		expect(
			readPgControlTarget(settings, {
				LOAMS_PG_CONTROL_URL: "http://127.0.0.1:17680",
				LOAMS_PG_CONTROL_API_KEY: "lk_env",
			}),
		).toEqual({ url: "http://127.0.0.1:17680", apiKey: "lk_env" });
	});

	it("ignores a URL that is not http(s)", () => {
		expect(
			readPgControlTarget(join(dir, "missing.json"), {
				LOAMS_PG_CONTROL_URL: "file:///etc/passwd",
			}).url,
		).toBe(DEFAULT_PG_CONTROL_URL);
	});
});
