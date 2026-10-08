import { isAbsolute, join, posix, relative } from "node:path";

const MIME: Record<string, string> = {
	".html": "text/html; charset=utf-8",
	".js": "text/javascript; charset=utf-8",
	".mjs": "text/javascript; charset=utf-8",
	".css": "text/css; charset=utf-8",
	".json": "application/json; charset=utf-8",
	".map": "application/json; charset=utf-8",
	".svg": "image/svg+xml",
	".png": "image/png",
	".jpg": "image/jpeg",
	".jpeg": "image/jpeg",
	".gif": "image/gif",
	".webp": "image/webp",
	".ico": "image/x-icon",
	".woff": "font/woff",
	".woff2": "font/woff2",
	".ttf": "font/ttf",
	".txt": "text/plain; charset=utf-8",
	".wasm": "application/wasm",
};

export type StaticResult =
	| { file: string; mime: string }
	| { spaFallback: string }
	| null;

/** Maps a loams-app://console/ui/... URL onto a file inside distRoot. Pure: no fs access. */
export function resolveStatic(distRoot: string, url: URL): StaticResult {
	if (url.hostname !== "console") return null;
	let decoded: string;
	try {
		decoded = decodeURIComponent(url.pathname);
	} catch {
		return null;
	}
	if (decoded.includes("\0") || decoded.includes("\\")) return null;
	const norm = posix.normalize(decoded);
	if (!norm.startsWith("/ui/")) return null;
	// The vite build emits dist/<file> with base /ui/, so strip the prefix.
	const rel = relative(distRoot, join(distRoot, norm.slice("/ui/".length)));
	if (rel.startsWith("..") || isAbsolute(rel)) return null;

	const base = posix.basename(norm);
	const ext = posix.extname(base).toLowerCase();
	if (!ext || norm.endsWith("/")) {
		const page = norm.startsWith("/ui/cordis") ? "cordis.html" : "index.html";
		return { spaFallback: join(distRoot, page) };
	}
	return {
		file: join(distRoot, rel),
		mime: MIME[ext] ?? "application/octet-stream",
	};
}
