// pg-control's settings, shared by main (which saves and uses them) and the
// Settings page (which checks a URL as it is typed). Pure: no node, no electron.

/** pg-control's API listener as the release compose stack publishes it. */
export const DEFAULT_PG_CONTROL_URL = "http://127.0.0.1:7680";

export type ParsedPgControlUrl =
	| { ok: true; url: string }
	| { ok: false; message: string };

/**
 * A pg-control URL as typed, as the origin main sends requests to. `127.0.0.1:7680`
 * reads as `http://127.0.0.1:7680`; an empty value is `""` (the default applies).
 * Only http and https, with a host and no user, path, query or fragment: pg-control
 * serves its API at the root of its listener.
 */
export function parsePgControlUrl(input: string): ParsedPgControlUrl {
	const raw = input.trim();
	if (raw === "") return { ok: true, url: "" };
	const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(raw)
		? raw
		: `http://${raw}`;
	let u: URL;
	try {
		u = new URL(withScheme);
	} catch {
		return {
			ok: false,
			message: "Enter a host and port, like 127.0.0.1:7680.",
		};
	}
	if (u.protocol !== "http:" && u.protocol !== "https:")
		return { ok: false, message: "Use an http:// or https:// URL." };
	if (!u.hostname)
		return {
			ok: false,
			message: "Enter a host and port, like 127.0.0.1:7680.",
		};
	if (u.username || u.password)
		return {
			ok: false,
			message: "Leave credentials out of the URL; use the API key file.",
		};
	if ((u.pathname !== "/" && u.pathname !== "") || u.search || u.hash)
		return {
			ok: false,
			message: "Use the listener's address only, without a path.",
		};
	return { ok: true, url: u.origin };
}
