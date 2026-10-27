import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import {
	readThemeChoice,
	THEME_SETTING,
	writeSetting,
} from "../src/main/settings";
import { THEME_ARG, themeFromArgv } from "../src/shared/version";

let dir: string | undefined;
afterEach(() => dir && rmSync(dir, { recursive: true, force: true }));

describe("the desktop's theme setting (D1074)", () => {
	it("defaults to system and survives a bad value", () => {
		dir = mkdtempSync(join(tmpdir(), "loams-theme-"));
		const file = join(dir, "settings.json");
		expect(readThemeChoice(file)).toBe("system");
		writeSetting(file, THEME_SETTING, "sepia");
		expect(readThemeChoice(file)).toBe("system");
		writeSetting(file, THEME_SETTING, "light");
		expect(readThemeChoice(file)).toBe("light");
		expect(JSON.parse(readFileSync(file, "utf8"))["ui.theme"]).toBe("light");
	});

	it("reaches the preload as an argument", () => {
		expect(themeFromArgv([`${THEME_ARG}dark`])).toBe("dark");
		expect(themeFromArgv([`${THEME_ARG}nonsense`])).toBe("system");
		expect(themeFromArgv([])).toBe("system");
	});
});
