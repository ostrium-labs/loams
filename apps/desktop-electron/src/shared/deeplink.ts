// loams:// deep links. Pure parser; the result is a console hash-route path
// (the cordis console uses a HashRouter, so `/servers` is `cordis.html#/servers`).

const SEG = /^[A-Za-z0-9._-]+$/;
const LINK = /^loams:\/\/open((?:\/[^?#]*)?)$/i;

function segments(rest: string): string[] | null {
	if (rest === "") return [];
	const parts = rest.slice(1).split("/");
	for (const p of parts) {
		if (!SEG.test(p) || p === "." || p === "..") return null;
	}
	return parts;
}

export function parseDeepLink(raw: string): { path: string } | null {
	const m = LINK.exec(raw);
	if (!m) return null;
	const parts = segments(m[1] ?? "");
	if (!parts || parts.length === 0) return null;
	const [head, ...rest] = parts;
	switch (head) {
		case "console":
			return rest.length > 0 ? { path: `/${rest.join("/")}` } : null;
		case "data":
			return { path: `/data${rest.map((s) => `/${s}`).join("")}` };
		case "factory":
			return rest.length <= 1
				? { path: `/factory${rest.map((s) => `/${s}`).join("")}` }
				: null;
		case "servers":
			return rest.length === 0 ? { path: "/servers" } : null;
		default:
			return null;
	}
}
