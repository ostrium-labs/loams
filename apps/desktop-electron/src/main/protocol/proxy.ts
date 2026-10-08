const PREFIXES = [
	"/api/",
	"/v1/",
	"/loams.",
	"/grpc.health.",
	"/.well-known/",
	"/health",
	"/ready",
];

export function isProxied(pathname: string): boolean {
	return PREFIXES.some((p) =>
		p.endsWith("/") || p.endsWith(".")
			? pathname.startsWith(p)
			: pathname === p || pathname.startsWith(`${p}/`),
	);
}

/**
 * Forwards a loams-app://console request to the active server's origin.
 * Redirects are returned as-is (never followed), cookie Domain attributes are
 * dropped so cookies bind to the loams-app host, and the API CSP is removed.
 */
export async function proxyRequest(
	req: Request,
	target: string,
	fetchImpl: typeof fetch,
): Promise<Response> {
	const src = new URL(req.url);
	const dest = new URL(target);
	const headers = new Headers(req.headers);
	headers.delete("origin");
	headers.delete("referer");
	headers.set("origin", dest.origin);
	const hasBody = req.method !== "GET" && req.method !== "HEAD";
	const out = new Request(`${dest.origin}${src.pathname}${src.search}`, {
		method: req.method,
		headers,
		body: hasBody ? req.body : undefined,
		redirect: "manual",
		duplex: "half",
	});
	const res = await fetchImpl(out);
	const rh = new Headers(res.headers);
	rh.delete("content-security-policy");
	const cookies = res.headers.getSetCookie();
	if (cookies.length > 0) {
		rh.delete("set-cookie");
		for (const c of cookies)
			rh.append("set-cookie", c.replace(/;\s*Domain=[^;]*/gi, ""));
	}
	return new Response(res.body, {
		status: res.status,
		statusText: res.statusText,
		headers: rh,
	});
}
