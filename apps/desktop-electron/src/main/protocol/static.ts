import { createHash } from "node:crypto";
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

/**
 * The console's CSP. Must equal `CSP` in web/apps/console/vite.config.ts, which the
 * production build writes into cordis.html (a test compares the two).
 */
export const CONSOLE_CSP =
	"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; " +
	"img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; frame-src 'self'; " +
	"object-src 'none'; base-uri 'none'; form-action 'self'";

const CSP_META = /<meta\b[^>]*http-equiv\s*=\s*["']?content-security-policy/i;

/** `'sha256-…'` sources for each inline `<script>` (same rule as deploy/stage.mjs). */
function inlineScriptHashes(html: string): string[] {
	const out: string[] = [];
	for (const m of html.matchAll(
		/<script\b([^>]*)>([\s\S]*?)<\/script\b[^>]*>/gi,
	)) {
		if (/\bsrc\s*=/i.test(m[1] ?? "")) continue;
		const digest = createHash("sha256")
			.update(m[2] ?? "", "utf8")
			.digest("base64");
		out.push(`'sha256-${digest}'`);
	}
	return out;
}

/**
 * Adds the console CSP `<meta>` to a served page unless it already has one.
 * Inline scripts already in the page (index.html's theme picker) are allowed by hash.
 */
export function ensureCspMeta(html: string): string {
	if (CSP_META.test(html)) return html;
	const hashes = inlineScriptHashes(html);
	const csp = hashes.length
		? CONSOLE_CSP.replace(
				"script-src 'self'",
				`script-src 'self' ${hashes.join(" ")}`,
			)
		: CONSOLE_CSP;
	const meta = `<meta http-equiv="Content-Security-Policy" content="${csp}" />`;
	const charset = /<meta charset=["']?utf-8["']?\s*\/?>/i;
	if (charset.test(html))
		return html.replace(charset, (m) => `${m}\n    ${meta}`);
	const head = /<head\b[^>]*>/i;
	if (head.test(html)) return html.replace(head, (m) => `${m}${meta}`);
	return `${meta}${html}`;
}
