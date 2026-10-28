// App settings > Postgres control plane (PG2 R9.1): main checks and saves the
// pg-control URL and the API key file, and tests them. The key is read only in
// main and goes only into the Authorization header of the request to pg-control.
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vite-plus/test";
import {
	checkPgControlInput,
	pgControlView,
	savePgControlSettings,
	testPgControl,
} from "../src/main/pgcontrol/settings";
import { readPgControlTarget } from "../src/main/pgcontrol/target";
import {
	DEFAULT_PG_CONTROL_URL,
	parsePgControlUrl,
} from "../src/shared/pg-control";

const KEY = "lk_01K7AAAAAAAAAAAAAAAAAAAAAA_abcdefghijklmnopqrstuvwxyz012345";

function dir() {
	const d = mkdtempSync(join(tmpdir(), "pgc-settings-"));
	const keyFile = join(d, "pg-api-key");
	writeFileSync(keyFile, `${KEY}\n`);
	return { d, keyFile, settings: join(d, "settings.json") };
}

const json = (status: number, body: unknown) =>
	new Response(JSON.stringify(body), {
		status,
		headers: { "content-type": "application/json" },
	});

describe("parsePgControlUrl", () => {
	it("reads host:port as http and keeps only the origin", () => {
		expect(parsePgControlUrl("127.0.0.1:7680")).toEqual({
			ok: true,
			url: "http://127.0.0.1:7680",
		});
		expect(parsePgControlUrl(" https://pg.example:443/ ")).toEqual({
			ok: true,
			url: "https://pg.example",
		});
		expect(parsePgControlUrl("")).toEqual({ ok: true, url: "" });
	});

	it("refuses what is not a listener address", () => {
		for (const bad of [
			"ftp://x:1",
			"http://user:pw@x:1",
			"http://x:1/api",
			"http://x:1/?a=1",
			"http://",
			"http://x:99999",
		])
			expect(parsePgControlUrl(bad).ok, bad).toBe(false);
	});
});

describe("checkPgControlInput", () => {
	it("accepts the default shape and reads the key in main", () => {
		const { keyFile } = dir();
		const r = checkPgControlInput({
			url: "127.0.0.1:7680",
			apiKeyFile: keyFile,
		});
		expect(r.errors).toEqual({});
		expect(r.value).toEqual({
			url: "http://127.0.0.1:7680",
			apiKeyFile: keyFile,
		});
		expect(r.key).toBe(KEY);
	});

	it("says what is wrong with each field", () => {
		const { d } = dir();
		const empty = join(d, "empty");
		writeFileSync(empty, "  \n");
		const twoLines = join(d, "two");
		writeFileSync(twoLines, "lk_a\nlk_b\n");
		const big = join(d, "big");
		writeFileSync(big, "x".repeat(5000));
		const sub = join(d, "sub");
		mkdirSync(sub);
		const cases: [string, RegExp][] = [
			["relative/key", /absolute path/],
			[join(d, "missing"), /No file/],
			[sub, /not a file/],
			[empty, /empty/],
			[twoLines, /one API key/],
			[big, /too large/],
		];
		for (const [apiKeyFile, msg] of cases)
			expect(
				checkPgControlInput({ url: "", apiKeyFile }).errors.apiKeyFile,
				apiKeyFile,
			).toMatch(msg);
		expect(
			checkPgControlInput({ url: "http://x:1/p", apiKeyFile: "" }).errors.url,
		).toMatch(/without a path/);
		// Not an object at all: both empty, which is valid (the defaults).
		expect(checkPgControlInput(42).errors).toEqual({});
	});
});

describe("savePgControlSettings", () => {
	it("saves, then requests use the new values without a restart", () => {
		const { settings, keyFile } = dir();
		writeFileSync(settings, JSON.stringify({ "ui.theme": "dark" }));
		expect(readPgControlTarget(settings, {})).toEqual({
			url: DEFAULT_PG_CONTROL_URL,
			apiKey: null,
		});
		const r = savePgControlSettings(
			settings,
			{ url: "10.0.0.5:7681", apiKeyFile: keyFile },
			{},
		);
		expect(r).toMatchObject({
			ok: true,
			value: {
				url: "http://10.0.0.5:7681",
				apiKeyFile: keyFile,
				effectiveUrl: "http://10.0.0.5:7681",
				keySource: "settings-file",
				envOverrides: [],
			},
		});
		// The key never reaches the renderer's view of the settings.
		expect(JSON.stringify(r)).not.toContain(KEY);
		expect(readPgControlTarget(settings, {})).toEqual({
			url: "http://10.0.0.5:7681",
			apiKey: KEY,
		});
		// Other settings are kept.
		expect(JSON.parse(readFileSync(settings, "utf8"))["ui.theme"]).toBe("dark");
	});

	it("empty values remove the settings, so the defaults apply again", () => {
		const { settings, keyFile } = dir();
		savePgControlSettings(
			settings,
			{ url: "10.0.0.5:7681", apiKeyFile: keyFile },
			{},
		);
		const r = savePgControlSettings(settings, { url: " ", apiKeyFile: "" }, {});
		expect(r).toMatchObject({
			ok: true,
			value: {
				url: "",
				apiKeyFile: "",
				effectiveUrl: DEFAULT_PG_CONTROL_URL,
				keySource: "none",
			},
		});
		const raw = JSON.parse(readFileSync(settings, "utf8"));
		expect("postgres.control.url" in raw).toBe(false);
		expect("postgres.control.apiKeyFile" in raw).toBe(false);
	});

	it("refuses invalid values and writes nothing", () => {
		const { settings, d } = dir();
		const r = savePgControlSettings(
			settings,
			{ url: "ftp://x", apiKeyFile: join(d, "missing") },
			{},
		);
		expect(r).toMatchObject({
			ok: false,
			code: "invalid_argument",
			errors: { url: expect.any(String), apiKeyFile: expect.any(String) },
		});
		expect(() => readFileSync(settings)).toThrow();
	});

	it("reports the environment variables that win", () => {
		const { settings, keyFile } = dir();
		savePgControlSettings(
			settings,
			{ url: "10.0.0.5:7681", apiKeyFile: keyFile },
			{},
		);
		const v = pgControlView(settings, {
			LOAMS_PG_CONTROL_URL: "http://127.0.0.1:9999",
			LOAMS_PG_CONTROL_API_KEY: "lk_env",
		});
		expect(v).toMatchObject({
			url: "http://10.0.0.5:7681",
			effectiveUrl: "http://127.0.0.1:9999",
			envOverrides: ["LOAMS_PG_CONTROL_URL", "LOAMS_PG_CONTROL_API_KEY"],
			keySource: "env",
		});
		expect(JSON.stringify(v)).not.toContain("lk_env");
	});
});

describe("testPgControl", () => {
	it("calls ListOperations with the form's values and the key from the file", async () => {
		const { keyFile } = dir();
		const fetchImpl = vi.fn(
			async (_u: string | URL | Request, _i?: RequestInit) =>
				json(200, { operations: [] }),
		);
		const r = await testPgControl(
			{ url: "127.0.0.1:7690", apiKeyFile: keyFile },
			fetchImpl as unknown as typeof fetch,
			{},
		);
		expect(r).toMatchObject({
			ok: true,
			code: "ok",
			url: "http://127.0.0.1:7690",
		});
		expect(r.message).toMatch(/accepted the API key/);
		const [u, init] = fetchImpl.mock.calls[0] ?? [];
		expect(String(u)).toBe(
			"http://127.0.0.1:7690/loams.operations.v1.OperationsService/ListOperations",
		);
		const headers = init?.headers as Record<string, string>;
		expect(headers.authorization).toBe(`Bearer ${KEY}`);
		expect(headers["connect-protocol-version"]).toBe("1");
		expect(JSON.stringify(r)).not.toContain(KEY);
	});

	it("tells a refused key, a refusal, a wrong service and an error apart", async () => {
		const { keyFile } = dir();
		const run = (res: Response, apiKeyFile = keyFile) =>
			testPgControl(
				{ url: "", apiKeyFile },
				(async () => res) as unknown as typeof fetch,
				{},
			);
		expect(
			await run(
				json(401, { code: "unauthenticated", message: `bad key ${KEY}` }),
			),
		).toMatchObject({
			ok: false,
			code: "unauthenticated",
			url: DEFAULT_PG_CONTROL_URL,
		});
		expect(await run(json(401, { code: "unauthenticated" }), "")).toMatchObject(
			{
				code: "unauthenticated",
				message: expect.stringMatching(/needs an API key/),
			},
		);
		const denied = await run(
			json(403, { code: "permission_denied", message: `no for ${KEY}` }),
		);
		expect(denied.code).toBe("permission_denied");
		// Even an echoed key is scrubbed from the message.
		expect(denied.message).not.toContain(KEY);
		expect(denied.message).toContain("[redacted]");
		expect(await run(new Response("not found", { status: 404 }))).toMatchObject(
			{
				code: "not_pg_control",
			},
		);
		expect(
			await run(json(500, { code: "internal", message: "boom" })),
		).toMatchObject({
			code: "error",
			message: expect.stringMatching(/HTTP 500: boom/),
		});
	});

	it("says when nothing answers, or not in time", async () => {
		const refused = await testPgControl(
			{ url: "127.0.0.1:1", apiKeyFile: "" },
			(async () => {
				throw new TypeError("fetch failed", {
					cause: new Error("connect ECONNREFUSED"),
				});
			}) as unknown as typeof fetch,
			{},
		);
		expect(refused).toMatchObject({ ok: false, code: "unreachable" });
		expect(refused.message).toMatch(/ECONNREFUSED/);
		const slow = await testPgControl(
			{ url: "127.0.0.1:1", apiKeyFile: "" },
			((_u: string, init: RequestInit) =>
				new Promise((_r, reject) =>
					init.signal?.addEventListener("abort", () =>
						reject(init.signal?.reason),
					),
				)) as unknown as typeof fetch,
			{},
			20,
		);
		expect(slow).toMatchObject({ code: "unreachable" });
		expect(slow.message).toMatch(/No answer/);
	});

	it("checks the values first, and the environment still wins", async () => {
		const fetchImpl = vi.fn(async () => json(200, {}));
		const bad = await testPgControl(
			{ url: "nope://", apiKeyFile: "" },
			fetchImpl as unknown as typeof fetch,
			{},
		);
		expect(bad).toMatchObject({
			ok: false,
			code: "invalid",
			errors: { url: expect.any(String) },
		});
		expect(fetchImpl).not.toHaveBeenCalled();
		await testPgControl(
			{ url: "127.0.0.1:7690", apiKeyFile: "" },
			fetchImpl as unknown as typeof fetch,
			{
				LOAMS_PG_CONTROL_URL: "http://127.0.0.1:9999",
				LOAMS_PG_CONTROL_API_KEY: "lk_env",
			},
		);
		const [u, init] = (fetchImpl.mock.calls[0] ?? []) as unknown as [
			string,
			RequestInit,
		];
		expect(u).toMatch(/^http:\/\/127\.0\.0\.1:9999\//);
		expect((init.headers as Record<string, string>).authorization).toBe(
			"Bearer lk_env",
		);
	});
});
