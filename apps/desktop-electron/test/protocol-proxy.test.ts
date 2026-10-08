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
		expect(res.headers.get("content-security-policy")).toBeNull();
	});
});
