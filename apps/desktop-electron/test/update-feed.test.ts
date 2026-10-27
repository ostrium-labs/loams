// Dry run of the update feed (desktop-latest): a release directory as the release workflow leaves it,
// signed with a throwaway key by sign-manifest.mjs, staged by update-feed.mjs the way
// desktop-promote.yml does, then read back through the app's own feed, signature and pin code.
import { execFileSync, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import * as ed from "@noble/ed25519";
import { afterAll, beforeAll, describe, expect, it } from "vite-plus/test";
import { createController, type UpdaterLike } from "../src/main/update/controller";
import { fetchFeedFiles } from "../src/main/update/feed";
import { verifyManifest } from "../src/main/update/manifest";
import { downloadedFileMatches, parsePinned } from "../src/main/update/pin";
import { channelFile } from "../src/main/update/policy";
// @ts-expect-error plain .mjs helper without types
import * as feed from "../scripts/update-feed.mjs";

const scripts = resolve(__dirname, "..", "scripts");
const scratchRoot = resolve(__dirname, "..", ".scratch");
let scratch = "";
const fresh = (name: string) => {
	const d = join(scratch, name);
	rmSync(d, { recursive: true, force: true });
	mkdirSync(d, { recursive: true });
	return d;
};
beforeAll(() => {
	mkdirSync(scratchRoot, { recursive: true });
	scratch = mkdtempSync(join(scratchRoot, "feed-"));
});
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const b64sha512 = (b: Buffer | string) =>
	createHash("sha512").update(b).digest("base64");

async function throwawayKey(dir: string) {
	const seed = randomBytes(32);
	const pub = Buffer.from(await ed.getPublicKeyAsync(seed)).toString("hex");
	const pinned = join(dir, "update-pubkey.txt");
	writeFileSync(pinned, `# test key\n${pub}\n`);
	return { seedHex: seed.toString("hex"), pub, pinned };
}

const V = "0.1.3";
const payloads: Record<string, string[]> = {
	"latest.yml": [`loams-desktop-${V}-win-x64.exe`],
	"latest-linux.yml": [`loams-desktop-${V}-linux-x86_64.AppImage`],
	"latest-linux-arm64.yml": [`loams-desktop-${V}-linux-arm64.AppImage`],
	"latest-mac.yml": [
		`loams-desktop-${V}-mac-arm64.zip`,
		`loams-desktop-${V}-mac-arm64.dmg`,
	],
};

/** A product release directory: payloads, other files, and electron-builder manifests (unsigned). */
function releaseDir(d: string, version = V) {
	writeFileSync(join(d, `loams-desktop-${V}-linux-amd64.deb`), "deb");
	writeFileSync(join(d, "DESKTOP-SHA256SUMS"), "sums");
	for (const [m, files] of Object.entries(payloads)) {
		const entries = files.map((f) => {
			const body = `payload ${f}`;
			writeFileSync(join(d, f), body);
			return { url: f, sha512: b64sha512(body), size: body.length };
		});
		const doc = {
			version,
			files: entries,
			path: entries[0]?.url,
			sha512: entries[0]?.sha512,
			releaseDate: "2026-10-26T00:00:00.000Z",
		};
		// The shape refresh-manifest.mjs (js-yaml dump) writes.
		const lines = [
			`version: ${doc.version}`,
			"files:",
			...doc.files.flatMap((f) => [
				`  - url: ${f.url}`,
				`    sha512: ${f.sha512}`,
				`    size: ${f.size}`,
			]),
			`path: ${doc.path}`,
			`sha512: ${doc.sha512}`,
			`releaseDate: '${doc.releaseDate}'`,
		];
		writeFileSync(join(d, m), `${lines.join("\n")}\n`);
	}
}

function signAll(d: string, seedHex: string) {
	for (const m of readdirSync(d).filter((n) => /^latest.*\.yml$/.test(n)))
		execFileSync("node", [join(scripts, "sign-manifest.mjs"), join(d, m)], {
			env: { ...process.env, LOAMS_UPDATE_SIGNING_KEY: seedHex },
		});
}

const cli = (args: string[], env: Record<string, string> = {}) =>
	spawnSync("node", [join(scripts, "update-feed.mjs"), ...args], {
		env: { ...process.env, ...env },
		encoding: "utf8",
	});

describe("update-feed.mjs keys", () => {
	it("derives the same public key as the app's Ed25519 library", async () => {
		const seed = randomBytes(32);
		expect(feed.pubkeyOfSeed(seed.toString("hex"))).toBe(
			Buffer.from(await ed.getPublicKeyAsync(seed)).toString("hex"),
		);
	});
	it("the committed update-pubkey.txt is empty or one 64-hex key", () => {
		const k = feed.readPinned(resolve(__dirname, "..", "update-pubkey.txt"));
		expect(k === "" || /^[0-9a-f]{64}$/.test(k)).toBe(true);
	});
	it("check-key passes for the pinned key's seed and fails for any other", async () => {
		const k = await throwawayKey(fresh("keys"));
		const env = { LOAMS_UPDATE_PUBKEY_FILE: k.pinned };
		expect(
			cli(["check-key"], { ...env, LOAMS_UPDATE_SIGNING_KEY: k.seedHex }).status,
		).toBe(0);
		const other = cli(["check-key"], {
			...env,
			LOAMS_UPDATE_SIGNING_KEY: randomBytes(32).toString("hex"),
		});
		expect(other.status).toBe(1);
		expect(other.stderr).toMatch(/not the private half/);
	});
	it("an empty pinned file means no key, and check-key refuses", () => {
		const d = fresh("empty");
		const pinned = join(d, "update-pubkey.txt");
		writeFileSync(pinned, "# nothing yet\n");
		expect(feed.readPinned(pinned)).toBe("");
		expect(cli(["pinned"], { LOAMS_UPDATE_PUBKEY_FILE: pinned }).stdout).toBe("");
		expect(
			cli(["check-key"], {
				LOAMS_UPDATE_PUBKEY_FILE: pinned,
				LOAMS_UPDATE_SIGNING_KEY: randomBytes(32).toString("hex"),
			}).status,
		).toBe(1);
	});
});

describe("feed generation dry run", () => {
	it("stages a signed product release and the app accepts what it serves", async () => {
		const root = fresh("dry");
		const k = await throwawayKey(root);
		const rel = join(root, "release");
		const out = join(root, "feed");
		mkdirSync(rel);
		releaseDir(rel);
		signAll(rel, k.seedHex);

		// What desktop-promote.yml downloads: manifests, signatures, then the payloads they name.
		const names = cli(["payloads", rel]).stdout.trim().split("\n");
		expect(names).toEqual(Object.values(payloads).flat().sort());

		const r = cli(["stage", rel, out], { LOAMS_UPDATE_PUBKEY_FILE: k.pinned });
		expect(r.stderr).toBe("");
		expect(r.status).toBe(0);
		const served = readdirSync(out).sort();
		expect(served).toEqual(
			[
				...names,
				...Object.keys(payloads),
				...Object.keys(payloads).map((m) => `${m}.sig`),
			].sort(),
		);
		expect(served).not.toContain("DESKTOP-SHA256SUMS");

		// Serve `out` the way GitHub does: the release URL redirects to an asset host.
		const base = "https://github.com/ostrium-labs/loams/releases/download/desktop-latest";
		const fetchFn = async (url: string) => {
			const u = new URL(url);
			if (u.hostname === "github.com")
				return new Response(null, {
					status: 302,
					headers: {
						location: `https://release-assets.githubusercontent.com/${u.pathname.split("/").pop()}`,
					},
				});
			const p = join(out, u.pathname.slice(1));
			return existsSync(p)
				? new Response(readFileSync(p))
				: new Response("nope", { status: 404 });
		};
		for (const [platform, arch] of [
			["win32", "x64"],
			["linux", "x64"],
			["linux", "arm64"],
			["darwin", "arm64"],
		] as const) {
			const file = channelFile(platform, arch);
			const files = await fetchFeedFiles(base, file, fetchFn);
			const sig = new TextDecoder().decode(files.sig);
			expect(await verifyManifest(files.yml, sig, k.pub), file).toBe(true);
			const pinned = parsePinned(files.yml);
			expect(pinned?.version).toBe(V);
			// The installer the updater would download matches what the manifest pins.
			const first = pinned?.files[0]?.url ?? "";
			const hash = async (f: string) => b64sha512(readFileSync(f));
			expect(
				await downloadedFileMatches(pinned!, join(out, first), hash),
			).toBe(true);
			// The dependency-free reader agrees with the app's js-yaml one.
			const mine = feed.parseManifest(new TextDecoder().decode(files.yml));
			expect(mine.version).toBe(pinned?.version);
			expect(mine.files.map((f: { url: string; sha512: string }) => [f.url, f.sha512])).toEqual(
				pinned?.files.map((f) => [f.url, f.sha512]),
			);
		}

		// And the update state machine offers the version (manual mode: no electron-updater calls).
		let state: unknown;
		const ctl = createController({
			mode: "manual",
			appVersion: "0.1.2",
			updater: { on: () => undefined } as unknown as UpdaterLike,
			fetchFeed: () => fetchFeedFiles(base, "latest-linux.yml", fetchFn),
			verify: (yml, s) => verifyManifest(yml, s, k.pub),
			hashFile: async () => "",
			deleteFile: async () => undefined,
			openRelease: async () => undefined,
			onState: (s) => {
				state = s;
			},
		});
		await ctl.check({ manual: true });
		expect(state).toMatchObject({ phase: "available", version: V });
	});

	const failing: Array<[string, (rel: string, seedHex: string) => void, RegExp]> = [
		[
			"a manifest without a signature",
			(rel) => rmSync(join(rel, "latest-linux.yml.sig")),
			/latest-linux\.yml has no latest-linux\.yml\.sig/,
		],
		[
			"a manifest signed by another key",
			(rel) => signAll(rel, randomBytes(32).toString("hex")),
			/does not verify against the pinned key/,
		],
		[
			"a manifest edited after signing",
			(rel) => {
				const p = join(rel, "latest.yml");
				writeFileSync(p, readFileSync(p, "utf8").replace(V, "9.9.9"));
			},
			/latest\.yml\.sig does not verify/,
		],
		[
			"a payload that differs from its manifest",
			(rel) => writeFileSync(join(rel, `loams-desktop-${V}-win-x64.exe`), "evil"),
			/win-x64\.exe: sha512 differs/,
		],
		[
			"a payload that is missing",
			(rel) => rmSync(join(rel, `loams-desktop-${V}-linux-arm64.AppImage`)),
			/which is not in/,
		],
	];
	for (const [name, damage, err] of failing)
		it(`refuses ${name}, staging nothing`, async () => {
			const root = fresh(`bad-${name.replace(/\W+/g, "-")}`);
			const k = await throwawayKey(root);
			const rel = join(root, "release");
			const out = join(root, "feed");
			mkdirSync(rel);
			releaseDir(rel);
			signAll(rel, k.seedHex);
			damage(rel, k.seedHex);
			const r = cli(["stage", rel, out], { LOAMS_UPDATE_PUBKEY_FILE: k.pinned });
			expect(r.status).toBe(1);
			expect(r.stderr).toMatch(err);
			expect(existsSync(out)).toBe(false);
		});

	it("refuses manifests of different versions and payload paths", async () => {
		const root = fresh("mixed");
		const k = await throwawayKey(root);
		const rel = join(root, "release");
		mkdirSync(rel);
		releaseDir(rel);
		const p = join(rel, "latest-linux.yml");
		writeFileSync(p, readFileSync(p, "utf8").replace(`version: ${V}`, "version: 0.1.4"));
		signAll(rel, k.seedHex);
		expect(() => feed.stageFeed(rel, join(root, "out"), k.pub)).toThrow(
			/disagree on the version/,
		);
		writeFileSync(
			join(rel, "latest.yml"),
			`version: ${V}\nfiles:\n  - url: ../../etc/passwd\n    sha512: x\n`,
		);
		expect(() => feed.payloadNames(rel)).toThrow(/not a plain file name/);
	});

	it("refuses to stage without a pinned key and with no manifests", () => {
		const d = fresh("nokey");
		expect(() => feed.stageFeed(d, join(d, "out"), "")).toThrow(/no pinned update public key/);
		expect(() => feed.stageFeed(d, join(d, "out"), "a".repeat(64))).toThrow(/no latest/);
	});
});
