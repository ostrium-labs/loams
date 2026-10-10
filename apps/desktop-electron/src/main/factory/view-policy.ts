// Pure policy for factory app windows: no electron import, unit-testable.
import type { NavDecision } from "../security/policy";

function parse(raw: string): URL | null {
	try {
		return new URL(raw);
	} catch {
		return null;
	}
}

const isWeb = (u: URL): boolean =>
	u.protocol === "http:" || u.protocol === "https:";

/** The origin of an http(s) URL, or undefined for anything else. */
export function webOrigin(raw: string): string | undefined {
	const u = parse(raw);
	return u && isWeb(u) ? u.origin : undefined;
}

/**
 * Same origin as the app (or the configured IdP origin) stays in the window;
 * any other http(s) URL goes to the system browser; everything else is denied.
 */
export function viewNavigation(
	appOrigin: string,
	to: string,
	ssoOrigin?: string,
): NavDecision {
	const u = parse(to);
	if (!u || !isWeb(u)) return "deny";
	const trusted = new Set<string>();
	const app = webOrigin(appOrigin);
	if (app) trusted.add(app);
	const sso = ssoOrigin ? webOrigin(ssoOrigin) : undefined;
	if (sso) trusted.add(sso);
	// userinfo in a URL is a spoofing vector: never keep it in-window.
	if (trusted.has(u.origin) && u.username === "" && u.password === "")
		return "allow";
	return "external";
}

/** Only sanitized clipboard writes, and only from the app's own origin. */
export function viewPermission(
	permission: string,
	requestingUrl: string,
	appOrigin: string,
): boolean {
	return (
		permission === "clipboard-sanitized-write" &&
		webOrigin(requestingUrl) !== undefined &&
		webOrigin(requestingUrl) === webOrigin(appOrigin)
	);
}

/**
 * Navigation decision per frame. Subframes may stay on the app/IdP origin; any
 * other target is blocked, never handed to the system browser.
 */
export function frameNavigation(
	appOrigin: string,
	to: string,
	isMainFrame: boolean,
	ssoOrigin?: string,
): NavDecision {
	const d = viewNavigation(appOrigin, to, ssoOrigin);
	if (isMainFrame) return d;
	return d === "allow" ? "allow" : "deny";
}

/** Downloads are allowed only from the app origin (or the configured IdP). */
export function downloadDecision(
	appOrigin: string,
	url: string,
	ssoOrigin?: string,
): "allow" | "deny" {
	return viewNavigation(appOrigin, url, ssoOrigin) === "allow"
		? "allow"
		: "deny";
}
