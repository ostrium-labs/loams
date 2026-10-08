// Pure tray model: no electron import, so it is unit-testable.
import type { EngineState, ServerEntry } from "../../shared/contracts";

export interface TrayItem {
	id: string;
	label: string;
	enabled: boolean;
}

/** Badge text: empty for none, capped at 99+. */
export function badgeLabel(n: number): string {
	const c = Number.isFinite(n) ? Math.max(0, Math.floor(n)) : 0;
	if (c === 0) return "";
	return c > 99 ? "99+" : String(c);
}

export function trayModel(
	engine: EngineState,
	badge: number,
	server: ServerEntry,
): { tooltip: string; items: TrayItem[] } {
	const n = Math.max(0, Math.floor(badge) || 0);
	let engineLabel: string;
	let engineEnabled = true;
	switch (engine.phase) {
		case "ready":
			engineLabel = "Engine: ready (stop)";
			break;
		case "starting":
			engineLabel = "Engine: starting";
			engineEnabled = false;
			break;
		case "failed":
			engineLabel = "Engine: failed (start)";
			break;
		default:
			engineLabel = "Engine: stopped (start)";
	}
	const tooltip = [
		"Loams Desktop",
		`engine ${engine.phase}`,
		n > 0 ? `${n} pending approval${n === 1 ? "" : "s"}` : "",
	]
		.filter(Boolean)
		.join(" - ");
	return {
		tooltip,
		items: [
			{ id: "open", label: "Open Loams Desktop", enabled: true },
			{ id: "engine", label: engineLabel, enabled: engineEnabled },
			{ id: "approvals", label: `Pending approvals (${n})`, enabled: n > 0 },
			{ id: "servers", label: `Servers › ${server.name}`, enabled: true },
			{ id: "quit", label: "Quit", enabled: true },
		],
	};
}

/** Console hash routes the tray opens; tests pin them to the plugins' registered paths. */
export const TRAY_ROUTES = {
	approvals: "/approvals",
	servers: "/settings/servers",
} as const;

/** Default for `shell.closeToTray`: on for Windows only. */
export function closeToTrayDefault(platform: string): boolean {
	return platform === "win32";
}

export type CloseAction = "hide" | "close";

/** Close-to-tray when a tray exists and the setting (default: Windows only) allows; hide on macOS; otherwise close. */
export function closeAction(o: {
	platform: string;
	hasTray: boolean;
	/** The user's setting; undefined means the platform default. */
	closeToTray: boolean | undefined;
	quitting: boolean;
}): CloseAction {
	if (o.quitting) return "close";
	if (o.platform === "darwin") return "hide";
	return o.hasTray && (o.closeToTray ?? closeToTrayDefault(o.platform))
		? "hide"
		: "close";
}
