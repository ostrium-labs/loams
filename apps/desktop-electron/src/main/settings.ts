import { readFileSync, renameSync, writeFileSync } from "node:fs";
import type {
	LiveStoreChoice,
	StudioNotifyPrefs,
	ThemeChoice,
} from "../shared/contracts";

/** Reads one key from userData/settings.json; `fallback` when missing or unreadable. */
export function readSetting<T>(file: string, key: string, fallback: T): T {
	try {
		const raw = JSON.parse(readFileSync(file, "utf8")) as Record<
			string,
			unknown
		>;
		return key in raw ? (raw[key] as T) : fallback;
	} catch {
		return fallback;
	}
}

/** Sets one key in userData/settings.json, keeping the others (written atomically). */
export function writeSetting(file: string, key: string, value: unknown): void {
	let raw: Record<string, unknown> = {};
	try {
		const parsed = JSON.parse(readFileSync(file, "utf8")) as unknown;
		if (parsed && typeof parsed === "object" && !Array.isArray(parsed))
			raw = parsed as Record<string, unknown>;
	} catch {
		/* missing or unreadable: start afresh */
	}
	raw[key] = value;
	const tmp = `${file}.tmp`;
	writeFileSync(tmp, `${JSON.stringify(raw, null, 2)}\n`);
	renameSync(tmp, file);
}

/** Where the user chose to keep Live (ruling T23-8): `live.store` in settings.json. */
export const LIVE_STORE_KEY = "live.store";

/** The user's Live store choice; `embedded` unless they chose the TiKV stack. */
export function readLiveStore(file: string): LiveStoreChoice {
	return readSetting<unknown>(file, LIVE_STORE_KEY, "embedded") === "tikv-stack"
		? "tikv-stack"
		: "embedded";
}

/**
 * Which daemon notifications main shows (SD1 Task 28): `studio.notify` in
 * settings.json, mirrored from Settings › Studio. Both on until the person says.
 */
export const STUDIO_NOTIFY_KEY = "studio.notify";

export function parseNotifyPrefs(v: unknown): StudioNotifyPrefs | null {
	if (!v || typeof v !== "object" || Array.isArray(v)) return null;
	const { turnDone, approval } = v as Record<string, unknown>;
	return typeof turnDone === "boolean" && typeof approval === "boolean"
		? { turnDone, approval }
		: null;
}

export function readNotifyPrefs(file: string): StudioNotifyPrefs {
	return (
		parseNotifyPrefs(readSetting<unknown>(file, STUDIO_NOTIFY_KEY, null)) ?? {
			turnDone: true,
			approval: true,
		}
	);
}

/** The theme choice (D1074): `ui.theme` in settings.json, "system" by default. */
export const THEME_SETTING = "ui.theme";

export function readThemeChoice(file: string): ThemeChoice {
	const v = readSetting<unknown>(file, THEME_SETTING, "system");
	return v === "dark" || v === "light" ? v : "system";
}

/** The update the person chose to skip (D1085); automatic checks stay quiet about it. */
export const UPDATE_SKIPPED_KEY = "update.skippedVersion";
