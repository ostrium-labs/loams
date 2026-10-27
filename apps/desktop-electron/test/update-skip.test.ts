import { describe, expect, it, vi } from "vite-plus/test";
import {
	type ControllerDeps,
	createController,
} from "../src/main/update/controller";

/** A signed manifest's content as parsePinned reads it (pin.ts:27-51). */
const YML = [
	"version: 0.2.0",
	"files:",
	"  - url: Loams-Desktop-0.2.0.AppImage",
	"    sha512: abc",
	"path: Loams-Desktop-0.2.0.AppImage",
	"sha512: abc",
	"",
].join("\n");
const INFO = {
	version: "0.2.0",
	files: [{ url: "Loams-Desktop-0.2.0.AppImage", sha512: "abc" }],
	path: "Loams-Desktop-0.2.0.AppImage",
	sha512: "abc",
};

function deps(
	over: Partial<ControllerDeps> = {},
): ControllerDeps & { states: string[] } {
	const states: string[] = [];
	let skipped: string | undefined;
	return {
		states,
		mode: "manual",
		appVersion: "0.1.0",
		updater: {
			autoDownload: false,
			autoInstallOnAppQuit: false,
			checkForUpdates: vi.fn(),
			downloadUpdate: vi.fn(),
			quitAndInstall: vi.fn(),
			on: vi.fn(),
		},
		fetchFeed: async () => ({
			yml: new TextEncoder().encode(YML),
			sig: new Uint8Array(),
		}),
		verify: async () => true,
		hashFile: async () => "",
		deleteFile: async () => {},
		openRelease: async () => {},
		onState: (s) =>
			void states.push(`${s.phase}${s.version ? `:${s.version}` : ""}`),
		skipped: () => skipped,
		saveSkipped: (v) => {
			skipped = v;
		},
		...over,
	} as ControllerDeps & { states: string[] };
}

describe("skip this version (D1085)", () => {
	it("an automatic check stays quiet about a skipped version; a manual check offers it again", async () => {
		const d = deps();
		const ctl = createController(d);
		await ctl.check({ manual: false });
		expect(ctl.state()).toMatchObject({ phase: "available", version: "0.2.0" });
		ctl.skip("0.2.0");
		expect(ctl.state().phase).toBe("idle");
		await ctl.check({ manual: false });
		expect(ctl.state().phase).toBe("idle");
		await ctl.check({ manual: true });
		expect(ctl.state()).toMatchObject({ phase: "available", version: "0.2.0" });
	});

	it("skipping an older version does not hide a newer one", async () => {
		const d = deps();
		const ctl = createController(d);
		ctl.skip("0.1.5");
		await ctl.check({ manual: false });
		expect(ctl.state()).toMatchObject({ phase: "available", version: "0.2.0" });
	});

	it("a self-updating check stays quiet about a skipped version too", async () => {
		const d = deps({ mode: "self" });
		(d.updater.checkForUpdates as ReturnType<typeof vi.fn>).mockResolvedValue({
			isUpdateAvailable: true,
			updateInfo: INFO,
		});
		const ctl = createController(d);
		ctl.skip("0.2.0");
		await ctl.check({ manual: false });
		expect(ctl.state().phase).toBe("idle");
		await ctl.check({ manual: true });
		expect(ctl.state()).toMatchObject({ phase: "available", version: "0.2.0" });
	});

	it("never downloads before consent", async () => {
		const d = deps({ mode: "self" });
		(d.updater.checkForUpdates as ReturnType<typeof vi.fn>).mockResolvedValue({
			isUpdateAvailable: true,
			updateInfo: INFO,
		});
		const ctl = createController(d);
		await ctl.check({ manual: false });
		expect(ctl.state()).toMatchObject({ phase: "available", version: "0.2.0" });
		expect(d.updater.downloadUpdate).not.toHaveBeenCalled();
	});
});
