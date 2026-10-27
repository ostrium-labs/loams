// SD1 Task 5: the protocol route to the daemon's loams.agentd.v1 listener
// (design §53 §6.2). Main adds the token, drops the renderer's Origin and
// Cookie, refuses methods the renderer may not call, answers 503 without a
// daemon, never lets the token reach the renderer, and streams unbuffered.
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { join } from "node:path";
import { describe, expect, it, vi } from "vite-plus/test";
import { isAgentdPath, rendererMayCall } from "../src/main/agentd/allowlist";
import { AGENTD_RENDERER_METHODS } from "../src/main/agentd/allowlist.gen";
import { AgentdToken } from "../src/main/agentd/client";
import {
	type AgentdTarget,
	parseDiscovery,
	readAgentdTarget,
} from "../src/main/agentd/discover";
import { handleAgentdRequest } from "../src/main/protocol/agentd";
import { isProxied } from "../src/main/protocol/proxy";
import { routeAgentd } from "../src/main/protocol/route";

const TOKEN = "s3cr3t-token-value-43-characters-long-abcdef";
const LIST = "/loams.agentd.v1.HarnessService/ListHarnesses";
const WATCH = "/loams.agentd.v1.ThreadService/WatchTranscript";

const target = (url = "http://127.0.0.1:50218"): AgentdTarget => ({
	url,
	token: new AgentdToken(TOKEN),
});

const rendererRequest = (path: string, init: RequestInit = {}) =>
	new Request(`loams-app://console${path}`, {
		method: "POST",
		headers: {
			"content-type": "application/json",
			origin: "loams-app://console",
			cookie: "session=renderer",
			referer: "loams-app://console/ui/",
			authorization: "Bearer renderer-guess",
			"connect-protocol-version": "1",
		},
		body: "{}",
		...init,
	});

describe("the agentd allowlist", () => {
	it("is generated and covers the Studio surface", () => {
		expect(AGENTD_RENDERER_METHODS.size).toBeGreaterThan(50);
		expect(rendererMayCall(LIST)).toBe(true);
		expect(rendererMayCall(WATCH)).toBe(true);
		for (const path of AGENTD_RENDERER_METHODS)
			expect(path.startsWith("/loams.agentd.v1.")).toBe(true);
	});

	it("routes the agentd prefix before the server prefixes", () => {
		expect(isAgentdPath(LIST)).toBe(true);
		// `/loams.` alone would send it to the active server.
		expect(isProxied(LIST)).toBe(true);
		expect(isAgentdPath("/loams.instance.v1.InstanceService/GetInstance")).toBe(
			false,
		);
	});
});

describe("routeAgentd", () => {
	it("forwards an allowed method to the Connect listener", () => {
		expect(routeAgentd(LIST, () => "http://127.0.0.1:1")).toEqual({
			kind: "forward",
			target: "http://127.0.0.1:1",
		});
	});

	it("proxy_refuses_non_renderer_method", async () => {
		const lookup = vi.fn(() => target());
		const fetchImpl = vi.fn(async () => new Response("{}"));
		for (const path of [
			// No such method.
			"/loams.agentd.v1.ThreadService/Shutdown",
			// A path, not an exact method.
			`${LIST}/extra`,
			"/loams.agentd.v1.HarnessService/",
		]) {
			const res = await handleAgentdRequest(
				rendererRequest(path),
				lookup,
				fetchImpl as never,
			);
			expect(res.status, path).toBe(403);
			expect(await res.json()).toMatchObject({ code: "forbidden_method" });
		}
		expect(fetchImpl).not.toHaveBeenCalled();
		// The token is not even read for a refused method.
		expect(lookup).not.toHaveBeenCalled();
	});
});

describe("handleAgentdRequest", () => {
	it("proxy_adds_token_and_drops_origin", async () => {
		const fetchImpl = vi.fn(async (_r: Request) => new Response("{}"));
		const res = await handleAgentdRequest(
			rendererRequest(`${LIST}?x=1`),
			() => target(),
			fetchImpl as never,
		);
		expect(res.status).toBe(200);
		const sent = fetchImpl.mock.calls[0]?.[0] as Request;
		expect(sent.url).toBe(`http://127.0.0.1:50218${LIST}?x=1`);
		expect(sent.method).toBe("POST");
		expect(sent.headers.get("authorization")).toBe(`Bearer ${TOKEN}`);
		expect(sent.headers.get("origin")).toBeNull();
		expect(sent.headers.get("cookie")).toBeNull();
		expect(sent.headers.get("referer")).toBeNull();
		expect(sent.headers.get("connect-protocol-version")).toBe("1");
		expect(sent.headers.get("content-type")).toBe("application/json");
		expect(await sent.text()).toBe("{}");
	});

	it("proxy_503_when_daemon_absent", async () => {
		const fetchImpl = vi.fn(async () => new Response("{}"));
		const res = await handleAgentdRequest(
			rendererRequest(LIST),
			() => null,
			fetchImpl as never,
		);
		expect(res.status).toBe(503);
		expect(await res.json()).toMatchObject({ code: "agentd_unavailable" });
		expect(fetchImpl).not.toHaveBeenCalled();

		// A daemon that stopped between the lookup and the call.
		const refused = await handleAgentdRequest(
			rendererRequest(LIST),
			() => target(),
			(async () => {
				throw new TypeError("fetch failed");
			}) as never,
		);
		expect(refused.status).toBe(503);
		expect(await refused.json()).toMatchObject({ code: "agentd_unavailable" });
	});

	it("renderer_never_sees_token", async () => {
		// An upstream that echoes the request's credentials back.
		const echo = async (r: Request) =>
			new Response(JSON.stringify({ ok: true }), {
				status: 401,
				headers: {
					"content-type": "application/json",
					"x-echo": r.headers.get("authorization") ?? "",
					"www-authenticate": `Bearer realm="${TOKEN}"`,
					"set-cookie": `t=${TOKEN}`,
				},
			});
		const responses = [
			await handleAgentdRequest(
				rendererRequest(LIST),
				() => target(),
				echo as never,
			),
			await handleAgentdRequest(
				rendererRequest(LIST),
				() => null,
				echo as never,
			),
			await handleAgentdRequest(
				rendererRequest("/loams.agentd.v1.GitService/ListWorktrees"),
				() => target(),
				echo as never,
			),
		];
		for (const res of responses) {
			for (const [name, value] of res.headers) {
				expect(value, name).not.toContain(TOKEN);
			}
			expect(await res.text()).not.toContain(TOKEN);
		}
		// The token object never prints itself.
		const t = new AgentdToken(TOKEN);
		expect(String(t)).not.toContain(TOKEN);
		expect(JSON.stringify({ t })).not.toContain(TOKEN);
		// The preload API offers the renderer no way to read it.
		const preload = readFileSync(
			join(__dirname, "../src/preload/index.ts"),
			"utf8",
		);
		expect(preload).not.toMatch(
			/agentd\.token|TOKEN_FILE|bearer\(|AgentdToken/,
		);
		expect(preload).not.toMatch(/token\s*:/i);
	});

	it("stream_passes_10000_frames_under_bound", async () => {
		const FRAMES = 10_000;
		// A fake Connect listener: one 5-byte envelope plus a small JSON frame per
		// terminal frame, written as it is produced.
		const server = createServer((req, res) => {
			expect(req.headers.authorization).toBe(`Bearer ${TOKEN}`);
			res.writeHead(200, { "content-type": "application/connect+json" });
			let i = 0;
			const pump = () => {
				while (i < FRAMES) {
					const body = Buffer.from(
						JSON.stringify({ event: { data: { seq: i, data: "x" } } }),
					);
					const head = Buffer.alloc(5);
					head.writeUInt32BE(body.length, 1);
					i += 1;
					const more = res.write(Buffer.concat([head, body]));
					if (i % 100 === 0) return setImmediate(pump);
					if (!more) return res.once("drain", pump);
				}
				res.end();
			};
			pump();
		});
		await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
		const port = (server.address() as AddressInfo).port;
		try {
			const res = await handleAgentdRequest(
				rendererRequest(WATCH),
				() => target(`http://127.0.0.1:${port}`),
				fetch,
			);
			expect(res.status).toBe(200);
			const reader = (res.body as ReadableStream<Uint8Array>).getReader();
			let pending = Buffer.alloc(0);
			let frames = 0;
			let last = performance.now();
			const gaps: number[] = [];
			for (;;) {
				const { done, value } = await reader.read();
				if (done) break;
				const now = performance.now();
				pending = Buffer.concat([pending, Buffer.from(value)]);
				while (pending.length >= 5) {
					const length = pending.readUInt32BE(1);
					if (pending.length < 5 + length) break;
					pending = pending.subarray(5 + length);
					frames += 1;
					gaps.push(now - last);
					last = now;
				}
			}
			expect(frames).toBe(FRAMES);
			gaps.sort((a, b) => a - b);
			const p99 = gaps[Math.floor(gaps.length * 0.99)] ?? 0;
			expect(p99).toBeLessThan(50);
		} finally {
			server.close();
		}
	});
});

describe("readAgentdTarget", () => {
	const discovery = {
		protocol: 1,
		version: "0.2.1",
		pid: 41234,
		port: 50217,
		startedAtMs: 1,
		mode: "child",
		connectPort: 50218,
	};
	const files = (entries: Record<string, string>) => (path: string) => {
		const name = path.split(/[\\/]/).pop() ?? "";
		const text = entries[name];
		if (text === undefined) throw new Error("ENOENT");
		return text;
	};

	it("reads the Connect port and the token", () => {
		expect(parseDiscovery(JSON.stringify(discovery))?.connectPort).toBe(50218);
		const found = readAgentdTarget("/d", {
			readFile: files({
				"agentd.json": JSON.stringify(discovery),
				"agentd.token": `${TOKEN}\n`,
			}),
			pidAlive: () => true,
		});
		expect(found?.url).toBe("http://127.0.0.1:50218");
		expect(found?.token.bearer()).toBe(`Bearer ${TOKEN}`);
	});

	it("is null without a daemon, a token or a Connect port", () => {
		const alive = { pidAlive: () => true };
		expect(
			readAgentdTarget("/d", { ...alive, readFile: files({}) }),
		).toBeNull();
		expect(
			readAgentdTarget("/d", {
				...alive,
				readFile: files({ "agentd.json": JSON.stringify(discovery) }),
			}),
		).toBeNull();
		const { connectPort: _, ...older } = discovery;
		expect(
			readAgentdTarget("/d", {
				...alive,
				readFile: files({
					"agentd.json": JSON.stringify(older),
					"agentd.token": TOKEN,
				}),
			}),
		).toBeNull();
		expect(
			readAgentdTarget("/d", {
				pidAlive: () => false,
				readFile: files({
					"agentd.json": JSON.stringify(discovery),
					"agentd.token": TOKEN,
				}),
			}),
		).toBeNull();
		expect(
			parseDiscovery(JSON.stringify({ ...discovery, connectPort: 70000 })),
		).toBeNull();
	});
});
