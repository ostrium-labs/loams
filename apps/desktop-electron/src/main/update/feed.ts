export const MAX_MANIFEST_BYTES = 256 * 1024;

/** Read a response body, refusing more than `max` bytes (content-length and streamed). */
export async function readCapped(
	res: Response,
	max = MAX_MANIFEST_BYTES,
): Promise<Uint8Array> {
	const len = Number(res.headers.get("content-length") ?? 0);
	if (len > max) throw new Error("too_large");
	if (!res.body) return new Uint8Array();
	const reader = res.body.getReader();
	const chunks: Uint8Array[] = [];
	let total = 0;
	for (;;) {
		const { done, value } = await reader.read();
		if (done) break;
		total += value.length;
		if (total > max) {
			await reader.cancel().catch(() => undefined);
			throw new Error("too_large");
		}
		chunks.push(value);
	}
	const out = new Uint8Array(total);
	let o = 0;
	for (const c of chunks) {
		out.set(c, o);
		o += c.length;
	}
	return out;
}

export type FetchLike = (
	url: string,
	init: { redirect: "manual" },
) => Promise<Response>;

export interface FeedFiles {
	yml: Uint8Array;
	sig: Uint8Array;
}

export const MAX_REDIRECTS = 5;

/**
 * Redirect targets we follow: https only, and either the feed's own host,
 * github.com, or a *.githubusercontent.com asset host (GitHub release downloads
 * 302 to release-assets/objects.githubusercontent.com). The bytes are still
 * Ed25519-verified, so this only limits who we talk to.
 */
export function redirectAllowed(target: URL, feedHost: string): boolean {
	if (target.protocol !== "https:") return false;
	if (target.username || target.password) return false;
	const h = target.hostname.toLowerCase();
	return (
		h === feedHost.toLowerCase() ||
		h === "github.com" ||
		h.endsWith(".githubusercontent.com")
	);
}

/** GET `url`, following at most MAX_REDIRECTS allowed redirects by hand. */
export async function fetchFollowing(
	url: string,
	fetchFn: FetchLike,
	feedHost: string,
): Promise<Response> {
	let current = url;
	for (let hop = 0; ; hop++) {
		const res = await fetchFn(current, { redirect: "manual" });
		if (res.status < 300 || res.status >= 400 || res.status === 304) return res;
		await res.body?.cancel().catch(() => undefined);
		if (hop >= MAX_REDIRECTS) throw new Error("too_many_redirects");
		const loc = res.headers.get("location");
		let next: URL;
		try {
			if (!loc) throw new Error("no location");
			next = new URL(loc, current);
		} catch {
			throw new Error("redirect_refused");
		}
		if (!redirectAllowed(next, feedHost)) throw new Error("redirect_refused");
		current = next.toString();
	}
}

/** Fetch `<feed>/<file>` and `.sig`; allow-listed redirects only, cache-busted. Throws on any transport problem. */
export async function fetchFeedFiles(
	feed: string,
	file: string,
	fetchFn: FetchLike,
	nonce: () => string = () => String(Date.now()),
): Promise<FeedFiles> {
	const base = feed.replace(/\/+$/, "");
	const feedHost = new URL(base).hostname;
	const get = async (name: string): Promise<Uint8Array> => {
		const res = await fetchFollowing(
			`${base}/${name}?noCache=${nonce()}`,
			fetchFn,
			feedHost,
		);
		if (!res.ok) throw new Error(`status_${res.status}`);
		return readCapped(res);
	};
	const [yml, sig] = await Promise.all([
		get(file),
		get(`${file}.sig`).catch(() => new Uint8Array()),
	]);
	return { yml, sig };
}
