// The one secret redactor for main: factory errors, agent transcripts and SQL errors all use it.
import type { Secret } from "./factory/vault";

export const MASK = "[redacted]";

const escapeRe = (s: string): string =>
	s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * Replaces every known secret value (longest first) with `mask`. With `credentials`, also
 * strips the password from `scheme://user:pass@` URLs and `password=`/`password:` pairs, and
 * keeps a `user:***@` user name even when it equals the password.
 */
export function redact(
	text: string,
	secrets: readonly (string | Secret)[] = [],
	opts: { mask?: string; credentials?: boolean } = {},
): string {
	const mask = opts.mask ?? MASK;
	let out = text;
	if (opts.credentials)
		out = out
			.replace(/\b([a-z][a-z0-9+.-]*:\/\/[^\s:/@]*):[^\s@/]*@/gi, `$1:${mask}@`)
			.replace(
				/(password\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s,;&]+)/gi,
				`$1${mask}`,
			);
	const values = secrets
		.map((s) => (typeof s === "string" ? s : s.reveal()))
		.filter((v) => v.length > 0)
		.sort((a, b) => b.length - a.length);
	const keepUser = opts.credentials ? `(?!:${escapeRe(mask)}@)` : "";
	for (const v of values)
		out = out.replace(new RegExp(`${escapeRe(v)}${keepUser}`, "g"), mask);
	return out;
}
