import { readFileSync } from "node:fs";

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
