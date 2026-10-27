// Loams Studio in the tray and in OS notifications (SD1 Task 28; §53 §7, §50 §5.5).
// Pure: no electron import. main/agentd/studio-watch.ts folds the daemon's
// `WatchThreads`, `WatchSessions`, `WatchApprovals` and `WatchNotifications` into a
// `StudioSnapshot` and hands each notification to `shouldNotify`.
import type { StudioNotifyPrefs } from "../../shared/contracts";
import type { TrayItem } from "./tray-model";

/** What the tray shows of Studio: the threads, which run, which wait on an approval. */
export interface StudioSnapshot {
	/** False until a daemon answers; the tray then has no Studio block. */
	connected: boolean;
	/** Thread id → title (`WatchThreads`; archived threads left out). */
	threads: ReadonlyMap<string, string>;
	/** Threads whose session is working (`WatchSessions`, `SESSION_STATUS_WORKING`). */
	working: ReadonlySet<string>;
	/** Pending approvals (`WatchApprovals`); empty while the daemon has none or no G17. */
	approvals: readonly { id: string; chatId: string }[];
}

export const EMPTY_STUDIO: StudioSnapshot = {
	connected: false,
	threads: new Map(),
	working: new Set(),
	approvals: [],
};

const MAX_ROWS = 5;
const MAX_TITLE = 48;
const SEG = /^[A-Za-z0-9._-]{1,128}$/;
const safeId = (id: string | undefined): id is string =>
	id !== undefined && SEG.test(id) && id !== "." && id !== "..";
const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

// oxlint-disable-next-line no-control-regex -- control characters are what is removed
const CONTROL = /[\u0000-\u001f\u007f]+/g;

/** A title as one short line of plain text: no control characters, capped. */
function line(text: string, max: number): string {
	const clean = text.replace(CONTROL, " ").replace(/\s+/g, " ").trim();
	return clean.length > max ? `${clean.slice(0, max - 1).trimEnd()}…` : clean;
}

export function studioCounts(s: StudioSnapshot): {
	running: number;
	approvals: number;
} {
	return { running: s.working.size, approvals: s.approvals.length };
}

/** The tray's Studio items, or none while no daemon answers. */
export function studioTrayItems(s: StudioSnapshot): TrayItem[] {
	if (!s.connected) return [];
	const { running, approvals } = studioCounts(s);
	const parts: string[] = [];
	if (running > 0) parts.push(`${running} running`);
	if (approvals > 0) parts.push(plural(approvals, "approval"));
	const items: TrayItem[] = [
		{
			id: "studio-status",
			label: parts.length ? `Studio: ${parts.join(" · ")}` : "Studio: idle",
			enabled: false,
		},
	];
	// The threads that need the person first, then the running ones.
	const waiting = new Map<string, number>();
	for (const a of s.approvals)
		waiting.set(a.chatId, (waiting.get(a.chatId) ?? 0) + 1);
	const order = [
		...waiting.keys(),
		...[...s.working].filter((id) => !waiting.has(id)),
	];
	for (const id of order.filter(safeId).slice(0, MAX_ROWS)) {
		const title = line(s.threads.get(id) ?? "", MAX_TITLE) || "Untitled thread";
		const n = waiting.get(id);
		items.push({
			id: `studio-thread:${id}`,
			label: `${title}: ${n ? plural(n, "approval") : "running"}`,
			enabled: true,
		});
	}
	items.push({ id: "studio-open", label: "Open Studio", enabled: true });
	return items;
}

/** The tooltip's Studio part, or empty. */
export function studioTooltip(s: StudioSnapshot): string {
	if (!s.connected) return "";
	const { running, approvals } = studioCounts(s);
	return running || approvals
		? `Studio: ${running} running, ${plural(approvals, "approval")}`
		: "";
}

/** The console route a Studio tray item opens. */
export function studioTrayRoute(itemId: string): string | undefined {
	if (itemId === "studio-open") return "/studio";
	const prefix = "studio-thread:";
	if (!itemId.startsWith(prefix)) return undefined;
	const id = itemId.slice(prefix.length);
	return safeId(id) ? `/studio/t/${id}` : undefined;
}

// ---- notifications ----

export type StudioNoticeKind =
	| "turn_done"
	| "approval"
	| "question"
	| "error"
	| "group_ready"
	| "check_failed";

/** One `WatchNotifications` item (G19), as main reads it. */
export interface StudioNotice {
	id: string;
	kind: StudioNoticeKind;
	chatId?: string | undefined;
	groupId?: string | undefined;
	/** The thread's or group's title; plain text from the daemon. */
	title: string;
	createdAtMs: number;
}

const HEADLINE: Record<StudioNoticeKind, string> = {
	turn_done: "Turn finished",
	approval: "Approval needed",
	question: "Question waiting",
	error: "Turn failed",
	group_ready: "Parallel runs finished",
	check_failed: "Check failed",
};

/** Which of Settings › Studio's two choices covers a kind. */
const PREF: Record<StudioNoticeKind, keyof StudioNotifyPrefs> = {
	turn_done: "turnDone",
	error: "turnDone",
	group_ready: "turnDone",
	check_failed: "turnDone",
	approval: "approval",
	question: "approval",
};

/** The Studio link a notification opens: its thread, else its run group, else Studio. */
export function notificationRoute(n: StudioNotice): string {
	if (safeId(n.chatId)) return `/studio/t/${n.chatId}`;
	if (safeId(n.groupId)) return `/studio/g/${n.groupId}`;
	return "/studio";
}

export function notificationContent(n: StudioNotice): {
	title: string;
	body: string;
} {
	return { title: HEADLINE[n.kind], body: line(n.title, 200) };
}

/**
 * Whether to show an OS notification: the person wants this kind, it happened since
 * this watch began (the daemon replays its last 200), and the window is not in front.
 */
export function shouldNotify(
	n: StudioNotice,
	prefs: StudioNotifyPrefs,
	at: { focused: boolean; since: number },
): boolean {
	return prefs[PREF[n.kind]] && n.createdAtMs >= at.since && !at.focused;
}
