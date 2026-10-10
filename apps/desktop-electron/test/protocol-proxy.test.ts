import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { gzipSync } from "node:zlib";
import { describe, expect, it, vi } from "vitest";
import { isProxied, proxyRequest } from "../src/main/protocol/proxy";

describe("isProxied", () => {
	it("isProxied_prefixes", () => {
		for (const p of [
			"/api/v1/session",
			"/v1/namespaces/x/sql",
			"/loams.instance.v1.InstanceService/GetInstance",
			"/grpc.health.v1.Health/Check",
			"/.well-known/openid",
			"/health",
			"/ready",
		])
			expect(isProxied(p), p).toBe(true);
		for (const p of ["/ui/index.html", "/", "/apix", "/v1", "/healthz-x/ui"])
			expect(isProxied(p), p).toBe(false);
	});
});

describe("proxyRequest", () => {
	it("proxy_rewrites_url_and_origin", async () => {
		const fetchImpl = vi.fn(async (_r: Request) => new Response("ok"));
		const req = new Request("loams-app://console/api/v1/x?y=1", {
			method: "POST",
			headers: {
				origin: "loams-app://console",
				referer: "loams-app://console/ui/",
				"x-csrf-token": "t",
			},
			body: "hello",
		});
		await proxyRequest(req, "http://127.0.0.1:8084", fetchImpl as never);
		const sent = fetchImpl.mock.calls[0]?.[0] as Request;
		expect(sent.url).toBe("http://127.0.0.1:8084/api/v1/x?y=1");
		expect(sent.method).toBe("POST");
		expect(sent.redirect).toBe("manual");
		expect(sent.headers.get("origin")).toBe("http://127.0.0.1:8084");
		expect(sent.headers.get("referer")).toBeNull();
		expect(sent.headers.get("x-csrf-token")).toBe("t");
		expect(await sent.text()).toBe("hello");
	});

	it("proxy_does_not_follow_cross_origin_redirect", async () => {
		const fetchImpl = vi.fn(
			async () =>
				new Response(null, {
					status: 302,
					headers: { location: "https://idp.example/auth" },
				}),
		);
		const res = await proxyRequest(
			new Request("loams-app://console/api/v1/oidc/start"),
			"http://127.0.0.1:8084",
			fetchImpl as never,
		);
		expect(res.status).toBe(302);
		expect(res.headers.get("location")).toBe("https://idp.example/auth");
		expect(fetchImpl).toHaveBeenCalledTimes(1);
	});

	it("strips_cookie_domain", async () => {
		const h = new Headers({ "content-security-policy": "default-src 'none'" });
		h.append("set-cookie", "a=1; Domain=.example.com; Path=/; HttpOnly");
		h.append("set-cookie", "b=2; domain=x.io; Secure");
		const res = await proxyRequest(
			new Request("loams-app://console/api/v1/x"),
			"http://h",
			(async () => new Response("{}", { status: 200, headers: h })) as never,
		);
		expect(res.headers.getSetCookie()).toEqual([
			"a=1; Path=/; HttpOnly",
			"b=2; Secure",
		]);
		expect(res.headers.get("content-security-policy")).toBe(
			"default-src 'none'",
		);
	});

	it("strips_cookie_domain_on_redirect", async () => {
		const h = new Headers({
			location: "https://idp.example/cb",
			"content-security-policy": "default-src 'none'",
		});
		h.append("set-cookie", "s=1; Domain=.x; Path=/");
		const fetchImpl = vi.fn(
			async () => new Response(null, { status: 302, headers: h }),
		);
		const res = await proxyRequest(
			new Request("loams-app://console/api/v1/login"),
			"http://h",
			fetchImpl as never,
		);
		expect(res.status).toBe(302);
		expect(res.headers.get("location")).toBe("https://idp.example/cb");
		expect(res.headers.getSetCookie()).toEqual(["s=1; Path=/"]);
		expect(res.headers.get("content-security-policy")).toBe(
			"default-src 'none'",
		);
		expect(fetchImpl).toHaveBeenCalledTimes(1);
	});

	it("drops_encoding_length_and_hop_by_hop_headers", async () => {
		const h = new Headers({
			"content-type": "application/json",
			"content-encoding": "gzip",
			"content-length": "999",
			"transfer-encoding": "chunked",
			connection: "keep-alive, x-custom",
			"keep-alive": "timeout=5",
			"proxy-authenticate": "Basic",
			"proxy-connection": "keep-alive",
			te: "trailers",
			trailer: "x-t",
			upgrade: "h2c",
			"x-keep": "1",
		});
		const res = await proxyRequest(
			new Request("loams-app://console/api/v1/x"),
			"http://h",
			(async () => new Response("{}", { headers: h })) as never,
		);
		for (const k of [
			"content-encoding",
			"content-length",
			"transfer-encoding",
			"connection",
			"keep-alive",
			"proxy-authenticate",
			"proxy-connection",
			"te",
			"trailer",
			"upgrade",
		])
			expect(res.headers.get(k), k).toBeNull();
		expect(res.headers.get("x-keep")).toBe("1");
		expect(res.headers.get("x-content-type-options")).toBe("nosniff");
	});

	it("gzip_upstream_body_is_served_decoded_without_encoding_header", async () => {
		const body = JSON.stringify({ hello: "world".repeat(50) });
		const srv = createServer((_req, out) => {
			const gz = gzipSync(body);
			out.writeHead(200, {
				"content-type": "application/json",
				"content-encoding": "gzip",
				"content-length": String(gz.length),
			});
			out.end(gz);
		});
		await new Promise<void>((r) => srv.listen(0, "127.0.0.1", r));
		try {
			const port = (srv.address() as AddressInfo).port;
			const res = await proxyRequest(
				new Request("loams-app://console/api/v1/x"),
				`http://127.0.0.1:${port}`,
				fetch,
			);
			expect(res.headers.get("content-encoding")).toBeNull();
			expect(res.headers.get("content-length")).toBeNull();
			expect(await res.text()).toBe(body);
		} finally {
			srv.close();
		}
	});

	it("html_responses_are_sandboxed_and_nosniff", async () => {
		const res = await proxyRequest(
			new Request("loams-app://console/api/v1/page"),
			"http://h",
			(async () =>
				new Response("<script>alert(1)</script>", {
					headers: {
						"content-type": "text/html; charset=utf-8",
						"content-security-policy": "default-src 'self'",
					},
				})) as never,
		);
		expect(res.headers.get("content-security-policy")).toBe(
			"default-src 'self', sandbox",
		);
		expect(res.headers.get("x-content-type-options")).toBe("nosniff");
		const plain = await proxyRequest(
			new Request("loams-app://console/api/v1/page"),
			"http://h",
			(async () =>
				new Response("<p>", {
					headers: { "content-type": "application/xhtml+xml" },
				})) as never,
		);
		expect(plain.headers.get("content-security-policy")).toBe("sandbox");
		const json = await proxyRequest(
			new Request("loams-app://console/api/v1/x"),
			"http://h",
			(async () =>
				new Response("{}", {
					headers: { "content-type": "application/json" },
				})) as never,
		);
		expect(json.headers.get("content-security-policy")).toBeNull();
	});
});
