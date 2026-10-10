// Ties what electron-updater resolves and downloads to the Ed25519-verified manifest (D661).
import { load } from "js-yaml";

export interface PinnedFile {
	url: string;
	sha512: string;
	size?: number;
}
export interface Pinned {
	version: string;
	files: PinnedFile[];
	path?: string;
	sha512?: string;
}
/** The subset of electron-updater's UpdateInfo that we compare. */
export interface InfoLike {
	version: string;
	files?: ReadonlyArray<{ url: string; sha512: string; size?: number }>;
	path?: string;
	sha512?: string;
}

const str = (v: unknown): string | undefined =>
	typeof v === "string" && v ? v : undefined;

/** Parse the already signature-verified yml bytes. Null when unusable. */
export function parsePinned(yml: Uint8Array): Pinned | null {
	let doc: unknown;
	try {
		doc = load(new TextDecoder().decode(yml));
	} catch {
		return null;
	}
	if (!doc || typeof doc !== "object") return null;
	const d = doc as Record<string, unknown>;
	const version = str(d.version);
	if (!version || !Array.isArray(d.files) || d.files.length === 0) return null;
	const files: PinnedFile[] = [];
	for (const f of d.files) {
		const o = (f ?? {}) as Record<string, unknown>;
		const url = str(o.url);
		const sha512 = str(o.sha512);
		if (!url || !sha512) return null;
		files.push({
			url,
			sha512,
			size: typeof o.size === "number" ? o.size : undefined,
		});
	}
	return { version, files, path: str(d.path), sha512: str(d.sha512) };
}

/** Equal version and equal {url, sha512} sets (order-insensitive), plus legacy path/sha512 when present. */
export function matchesPinned(info: InfoLike, pinned: Pinned): boolean {
	if (info.version !== pinned.version) return false;
	const key = (f: { url: string; sha512: string }) => `${f.url}\n${f.sha512}`;
	const a = new Set((info.files ?? []).map(key));
	const b = new Set(pinned.files.map(key));
	if (a.size !== b.size || a.size === 0) return false;
	for (const k of a) if (!b.has(k)) return false;
	if (info.path !== pinned.path || info.sha512 !== pinned.sha512) return false;
	return true;
}

const basename = (p: string): string => {
	const last = p.split(/[\\/]/).pop() ?? "";
	try {
		return decodeURIComponent(last);
	} catch {
		return last;
	}
};

/** True only when the downloaded file's sha512 (base64) equals the pinned entry with the same basename. */
export async function downloadedFileMatches(
	pinned: Pinned,
	downloadedFile: string,
	hashFile: (file: string) => Promise<string>,
): Promise<boolean> {
	const name = basename(downloadedFile);
	const entry = pinned.files.find((f) => basename(f.url) === name);
	if (!entry) return false;
	try {
		return (await hashFile(downloadedFile)) === entry.sha512;
	} catch {
		return false;
	}
}
