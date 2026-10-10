// Adapted from dataelement/dsh-desktop (MIT), src/main/security-policy.ts.
// Pure policy: no electron import, so it is unit-testable.

export const APP_ORIGIN = "loams-app://console";
const APP_PREFIX = `${APP_ORIGIN}/`;

export type NavDecision = "allow" | "external" | "deny";

function parse(raw: string): URL | null {
	try {
		return new URL(raw);
	} catch {
		return null;
	}
}

/** True for loams-app://console exactly (no userinfo, port or lookalike host). */
function isConsoleUrl(u: URL): boolean {
	return (
		u.protocol === "loams-app:" &&
		u.hostname === "console" &&
		u.username === "" &&
		u.password === "" &&
		u.port === ""
	);
}

/** `from` is accepted for symmetry with will-navigate; the decision depends on `to`. */
export function navigationDecision(_from: string, to: string): NavDecision {
	const u = parse(to);
	if (!u) return "deny";
	if (isConsoleUrl(u)) return "allow";
	if (u.protocol === "http:" || u.protocol === "https:") return "external";
	return "deny";
}

const GRANTED = new Set(["clipboard-sanitized-write", "notifications"]);

export function permissionDecision(
	permission: string,
	origin: string,
): boolean {
	if (!GRANTED.has(permission)) return false;
	const u = parse(origin);
	return u !== null && isConsoleUrl(u);
}

export interface FrameLike {
	url: string;
	parent?: unknown;
}

/** Throws unless the IPC sender is the top-level console frame. */
export function assertTrustedSender(event: {
	senderFrame?: FrameLike | null;
}): void {
	const f = event.senderFrame;
	const u = f ? parse(f.url) : null;
	if (
		!f ||
		!u ||
		!f.url.startsWith(APP_PREFIX) ||
		!isConsoleUrl(u) ||
		(f.parent !== null && f.parent !== undefined)
	) {
		throw new Error("untrusted IPC sender");
	}
}
