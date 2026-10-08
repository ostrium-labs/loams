import { describe, expect, it, vi } from "vitest";
import { isProxied, proxyRequest } from "../src/main/protocol/proxy";
import { routeRequest } from "../src/main/protocol/route";
import type { EngineState, ServerEntry } from "../src/shared/contracts";

const local: ServerEntry = {
	id: "local",
	name: "This computer",
	kind: "local",
	url: "http://127.0.0.1:8084",
};
const remote: ServerEntry = {
	id: "r",
	name: "Prod",
	kind: "remote",
	url: "https://loams.example",
};
const ready = (liveUrl?: string): EngineState => ({
	phase: "ready",
	url: "http://127.0.0.1:8084",
	esUrl: "http://127.0.0.1:9200",
	flightUrl: "http://127.0.0.1:8815",
	durableUrl: "http://127.0.0.1:8001",
	...(liveUrl ? { liveUrl } : {}),
	pid: 1,
});

describe("routeRequest", () => {
	it("routes_durable_to_engine", () => {
		expect(routeRequest("/durable/", local, ready())).toEqual({
			kind: "forward",
			target: "http://127.0.0.1:8001",
			pathname: "/",
		});
		expect(routeRequest("/durable/promises", local, ready())).toMatchObject({
			target: "http://127.0.0.1:8001",
			pathname: "/promises",
		});
		expect(isProxied("/durable/")).toBe(true);
	});

	it("live_goes_to_live_url_or_503", () => {
		const p = "/loams.live.v1.LiveService/Ping";
		expect(routeRequest(p, local, ready("http://127.0.0.1:7000"))).toEqual({
			kind: "forward",
			target: "http://127.0.0.1:7000",
		});
	});

	it("live_without_engine_503", () => {
		const r = routeRequest("/loams.live.v1.X/Y", local, ready());
		expect(r).toMatchObject({
			kind: "reject",
			status: 503,
			body: { code: "live_not_running" },
		});
	});

	it("remote_server_has_no_durable", () => {
		expect(routeRequest("/durable/", remote, ready())).toMatchObject({
			kind: "reject",
			status: 404,
			body: { code: "not_available_remote" },
		});
		expect(routeRequest("/api/v1/x", remote, ready())).toEqual({
			kind: "forward",
			target: "https://loams.example",
		});
		// a remote server's live service is the remote's own
		expect(routeRequest("/loams.live.v1.X/Y", remote, ready())).toEqual({
			kind: "forward",
			target: "https://loams.example",
		});
	});

	it("other_paths_go_to_active_and_not_ready_is_503", () => {
		expect(routeRequest("/api/v1/x", local, ready())).toEqual({
			kind: "forward",
			target: "http://127.0.0.1:8084",
		});
		const starting: ServerEntry = { ...local, url: "" };
		expect(
			routeRequest("/durable/", starting, { phase: "starting", attempt: 1 }),
		).toMatchObject({
			status: 503,
			body: { code: "engine_not_ready" },
		});
		expect(
			routeRequest("/durable/", local, { phase: "stopped" }),
		).toMatchObject({
			status: 503,
		});
	});

	it("forward_keeps_query_and_rewrites_path_without_following_redirects", async () => {
		const fetchImpl = vi.fn(
			async (_r: Request) =>
				new Response(null, {
					status: 302,
					headers: { location: "https://x.example/" },
				}),
		);
		const req = new Request("loams-app://console/durable/?a=1", {
			method: "POST",
			body: "{}",
		});
		const res = await proxyRequest(
			req,
			"http://127.0.0.1:8001",
			fetchImpl as never,
			"/",
		);
		const sent = fetchImpl.mock.calls[0]?.[0] as Request;
		expect(sent.url).toBe("http://127.0.0.1:8001/?a=1");
		expect(sent.redirect).toBe("manual");
		expect(res.status).toBe(302);
	});
});
