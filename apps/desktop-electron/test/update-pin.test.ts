import { describe, expect, it } from "vitest";
import {
	createController,
	type UpdaterLike,
} from "../src/main/update/controller";
import { fetchFeedFiles, readCapped } from "../src/main/update/feed";
import {
	downloadedFileMatches,
	matchesPinned,
	parsePinned,
} from "../src/main/update/pin";

const yml = (v = "2.0.0", sha = "AAA") =>
	new TextEncoder().encode(
		`version: ${v}\nfiles:\n  - url: Loams-${v}.AppImage\n    sha512: ${sha}\n    size: 10\npath: Loams-${v}.AppImage\nsha512: ${sha}\n`,
	);
const pinned = parsePinned(yml()) as NonNullable<
	ReturnType<typeof parsePinned>
>;
const info = (v = "2.0.0", sha = "AAA") => ({
	version: v,
	files: [{ url: `Loams-${v}.AppImage`, sha512: sha, size: 10 }],
	path: `Loams-${v}.AppImage`,
	sha512: sha,
});

describe("pin", () => {
	it("parses", () => {
		expect(pinned.version).toBe("2.0.0");
		expect(pinned.files[0]?.sha512).toBe("AAA");
		expect(parsePinned(new TextEncoder().encode("::: not yaml ["))).toBeNull();
		expect(parsePinned(new TextEncoder().encode("version: 1\n"))).toBeNull();
	});
	it("mismatched_update_info_refused", () => {
		expect(matchesPinned(info(), pinned)).toBe(true);
		expect(matchesPinned(info("2.0.1"), pinned)).toBe(false);
		expect(matchesPinned(info("2.0.0", "BBB"), pinned)).toBe(false);
		expect(matchesPinned({ version: "2.0.0", files: [] }, pinned)).toBe(false);
	});
	it("downloaded_file_hash_mismatch_refused", async () => {
		const f = "/cache/Loams-2.0.0.AppImage";
		expect(await downloadedFileMatches(pinned, f, async () => "AAA")).toBe(
			true,
		);
		expect(await downloadedFileMatches(pinned, f, async () => "BBB")).toBe(
			false,
		);
		expect(
			await downloadedFileMatches(
				pinned,
				"/cache/Other.AppImage",
				async () => "AAA",
			),
		).toBe(false);
		expect(
			await downloadedFileMatches(pinned, f, async () => {
				throw new Error("io");
			}),
		).toBe(false);
	});
});

describe("feed", () => {
	it("caps_size", async () => {
		await expect(readCapped(new Response("x".repeat(100)), 50)).rejects.toThrow(
			"too_large",
		);
		expect((await readCapped(new Response("abc"), 50)).length).toBe(3);
	});
	it("no_redirects_and_cache_bust", async () => {
		const seen: string[] = [];
		const out = await fetchFeedFiles(
			"https://f.example/x/",
			"latest.yml",
			async (u, init) => {
				seen.push(u);
				expect(init.redirect).toBe("error");
				return new Response("v");
			},
			() => "N",
		);
		expect(seen.sort()).toEqual([
			"https://f.example/x/latest.yml.sig?noCache=N",
			"https://f.example/x/latest.yml?noCache=N",
		]);
		expect(out.yml.length).toBe(1);
	});
});

function harness(
	over: {
		verify?: boolean;
		resolved?: ReturnType<typeof info>;
		hash?: string;
	} = {},
) {
	const handlers: Record<string, (e: never) => void> = {};
	const calls = { download: 0, quit: 0, deleted: [] as string[], restart: 0 };
	let quitThrows = false;
	const updater = {
		autoDownload: true,
		autoInstallOnAppQuit: true,
		checkForUpdates: async () => ({
			isUpdateAvailable: true,
			updateInfo: over.resolved ?? info(),
		}),
		downloadUpdate: async () => {
			calls.download++;
		},
		quitAndInstall: () => {
			calls.quit++;
			if (quitThrows) throw new Error("boom");
		},
		on: (ev: string, cb: (e: never) => void) => {
			handlers[ev] = cb;
		},
	} as unknown as UpdaterLike;
	const states: string[] = [];
	const ctl = createController({
		mode: "self",
		appVersion: "1.0.0",
		updater,
		fetchFeed: async () => ({ yml: yml(), sig: new Uint8Array() }),
		verify: async () => over.verify ?? true,
		hashFile: async () => over.hash ?? "AAA",
		deleteFile: async (f) => {
			calls.deleted.push(f);
		},
		openRelease: async () => undefined,
		restartEngine: async () => {
			calls.restart++;
		},
		onState: (s) => states.push(s.phase + (s.message ? `:${s.message}` : "")),
	});
	const downloaded = async () => {
		(handlers["update-downloaded"] as (e: unknown) => void)({
			version: "2.0.0",
			downloadedFile: "/c/Loams-2.0.0.AppImage",
		});
		await new Promise((r) => setTimeout(r, 0));
	};
	return {
		ctl,
		updater,
		calls,
		states,
		downloaded,
		setQuitThrows: () => (quitThrows = true),
	};
}

describe("controller", () => {
	it("happy_path_check_download_verify_install", async () => {
		const h = harness();
		expect(h.updater.autoInstallOnAppQuit).toBe(false);
		await h.ctl.check();
		expect(h.ctl.state().phase).toBe("available");
		await h.ctl.download();
		expect(h.calls.download).toBe(1);
		await h.downloaded();
		expect(h.ctl.state().phase).toBe("ready");
		expect(h.updater.autoInstallOnAppQuit).toBe(true);
		await h.ctl.install();
		expect(h.calls.quit).toBe(1);
	});
	it("unsigned_feed_never_available", async () => {
		const h = harness({ verify: false });
		await h.ctl.check();
		expect(h.ctl.state()).toMatchObject({
			phase: "error",
			message: "unsigned_manifest",
		});
		expect(h.states).not.toContain("available");
	});
	it("resolved_info_differing_from_signed_is_refused", async () => {
		const h = harness({ resolved: info("2.0.0", "EVIL") });
		await h.ctl.check();
		expect(h.ctl.state()).toMatchObject({
			phase: "error",
			message: "unsigned_manifest",
		});
		expect(h.states).not.toContain("available");
	});
	it("install_blocked_until_hash_verified", async () => {
		const h = harness({ hash: "WRONG" });
		await h.ctl.check();
		await h.ctl.install(); // not ready
		await h.ctl.download();
		await h.downloaded();
		expect(h.ctl.state()).toMatchObject({
			phase: "error",
			message: "download_hash_mismatch",
		});
		expect(h.calls.deleted).toEqual(["/c/Loams-2.0.0.AppImage"]);
		expect(h.updater.autoInstallOnAppQuit).toBe(false);
		await h.ctl.install();
		expect(h.calls.quit).toBe(0);
	});
	it("feed_unreachable_hides_url", async () => {
		const h = harness();
		const ctl = createController({
			mode: "self",
			appVersion: "1.0.0",
			updater: h.updater,
			fetchFeed: async () => {
				throw new Error("ECONNREFUSED https://secret.example");
			},
			verify: async () => true,
			hashFile: async () => "",
			deleteFile: async () => undefined,
			openRelease: async () => undefined,
			onState: () => undefined,
		});
		await ctl.check();
		expect(ctl.state()).toEqual({
			phase: "error",
			message: "feed_unreachable",
			mode: "self",
		});
	});
	it("restarts_engine_when_quit_and_install_throws", async () => {
		const h = harness();
		await h.ctl.check();
		await h.ctl.download();
		await h.downloaded();
		h.setQuitThrows();
		await h.ctl.install();
		expect(h.calls.restart).toBe(1);
		expect(h.ctl.state().phase).toBe("error");
	});
	it("manual_mode_never_touches_updater", async () => {
		const h = harness();
		let opened = "";
		const ctl = createController({
			mode: "manual",
			appVersion: "1.0.0",
			updater: h.updater,
			fetchFeed: async () => ({ yml: yml(), sig: new Uint8Array() }),
			verify: async () => true,
			hashFile: async () => "",
			deleteFile: async () => undefined,
			openRelease: async (v) => {
				opened = v;
			},
			onState: () => undefined,
		});
		await ctl.check();
		await ctl.download();
		expect(opened).toBe("2.0.0");
		expect(h.calls.download).toBe(0);
		await ctl.install();
		expect(h.calls.quit).toBe(0);
	});
});
