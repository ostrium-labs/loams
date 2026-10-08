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
	init: { redirect: "error" },
) => Promise<Response>;

export interface FeedFiles {
	yml: Uint8Array;
	sig: Uint8Array;
}

/** Fetch `<feed>/<file>` and `.sig`; no redirects, cache-busted. Throws on any transport problem. */
export async function fetchFeedFiles(
	feed: string,
	file: string,
	fetchFn: FetchLike,
	nonce: () => string = () => String(Date.now()),
): Promise<FeedFiles> {
	const base = feed.replace(/\/+$/, "");
	const get = async (name: string): Promise<Uint8Array> => {
		const res = await fetchFn(`${base}/${name}?noCache=${nonce()}`, {
			redirect: "error",
		});
		if (!res.ok) throw new Error(`status_${res.status}`);
		return readCapped(res);
	};
	const [yml, sig] = await Promise.all([
		get(file),
		get(`${file}.sig`).catch(() => new Uint8Array()),
	]);
	return { yml, sig };
}
