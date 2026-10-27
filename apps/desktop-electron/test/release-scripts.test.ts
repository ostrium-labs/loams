// Release helpers (Task 31): checksums.mjs, refresh-manifest.mjs, gpg-sign.sh, windows-sign.cjs.
import { execFileSync, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vite-plus/test";
// @ts-expect-error plain .mjs helper without types
import { artifactNames, sha256sums } from "../scripts/checksums.mjs";
// @ts-expect-error plain .mjs helper without types
import { refreshManifest } from "../scripts/refresh-manifest.mjs";

const scripts = resolve(__dirname, "..", "scripts");
// Scratch under the worktree (never /tmp): git-ignored via the app's .gitignore entry for .scratch.
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
	scratch = mkdtempSync(join(scratchRoot, "rel-"));
});
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const sha = (
	b: Buffer | string,
	algo = "sha256",
	enc: "hex" | "base64" = "hex",
) => createHash(algo).update(b).digest(enc);

describe("checksums.mjs", () => {
	it("lists artifacts sorted, excluding signatures and itself", () => {
		const d = fresh("sums");
		writeFileSync(join(d, "b.deb"), "bbb");
		writeFileSync(join(d, "a.AppImage"), "aaa");
		writeFileSync(join(d, "a.AppImage.sig"), "sig");
		writeFileSync(join(d, "SHA256SUMS"), "old");
		writeFileSync(join(d, "SHA256SUMS.asc"), "old");
		mkdirSync(join(d, "sub"));
		expect(artifactNames(d)).toEqual(["a.AppImage", "b.deb"]);
		expect(sha256sums(d)).toBe(
			`${sha("aaa")}  a.AppImage\n${sha("bbb")}  b.deb\n`,
		);
	});
	it("cli writes SHA256SUMS and refuses an empty directory", () => {
		const d = fresh("cli");
		writeFileSync(join(d, "x.rpm"), "x");
		execFileSync("node", [join(scripts, "checksums.mjs"), d]);
		expect(readFileSync(join(d, "SHA256SUMS"), "utf8")).toBe(
			`${sha("x")}  x.rpm\n`,
		);
		const e = fresh("empty");
		expect(spawnSync("node", [join(scripts, "checksums.mjs"), e]).status).toBe(
			1,
		);
	});
});

describe("refresh-manifest.mjs", () => {
	it("recomputes sha512 and size and drops the stale blockmap", () => {
		const d = fresh("manifest");
		writeFileSync(join(d, "setup.exe"), "signed bytes");
		writeFileSync(join(d, "setup.exe.blockmap"), "stale");
		const yml = join(d, "latest.yml");
		writeFileSync(
			yml,
			"version: 1.0.0\nfiles:\n  - url: setup.exe\n    sha512: OLD\n    size: 1\npath: setup.exe\nsha512: OLD\nreleaseDate: '2026-01-01T00:00:00.000Z'\n",
		);
		expect(refreshManifest(yml, d)).toEqual(["setup.exe"]);
		const out = readFileSync(yml, "utf8");
		expect(out).toContain(`sha512: ${sha("signed bytes", "sha512", "base64")}`);
		expect(out).toContain("size: 12");
		expect(out).not.toContain("OLD");
		expect(existsSync(join(d, "setup.exe.blockmap"))).toBe(false);
	});
	it("fails when a listed file is missing", () => {
		const d = fresh("manifest-missing");
		const yml = join(d, "latest.yml");
		writeFileSync(
			yml,
			"version: 1.0.0\nfiles:\n  - url: gone.exe\n    sha512: x\n    size: 1\n",
		);
		expect(() => refreshManifest(yml, d)).toThrow(/gone\.exe/);
	});
});

describe("windows-sign.cjs", () => {
	const { jsignArgs } = createRequire(import.meta.url)(
		"../scripts/windows-sign.cjs",
	);
	it("never puts the password on the command line", () => {
		const args: string[] = jsignArgs(
			{ WINDOWS_SIGN_KEYSTORE: "k.p12", WINDOWS_SIGN_STOREPASS: "hunter2" },
			"a.exe",
		);
		expect(args.join(" ")).not.toContain("hunter2");
		expect(args[args.indexOf("--storepass") + 1]).toBe(
			"env:WINDOWS_SIGN_STOREPASS",
		);
	});
	it("prefers a password file", () => {
		const args: string[] = jsignArgs(
			{ WINDOWS_SIGN_KEYSTORE: "k", WINDOWS_SIGN_STOREPASS_FILE: "/run/pw" },
			"a.exe",
		);
		expect(args[args.indexOf("--storepass") + 1]).toBe("file:/run/pw");
	});
	it("fails clearly without a password", () => {
		expect(() => jsignArgs({ WINDOWS_SIGN_KEYSTORE: "k" }, "a.exe")).toThrow(
			/STOREPASS/,
		);
	});
});

// The bundled binaries' SHA256SUMS, which `loams-agentd install-runtime` checks (release fix B9).
describe("bin-sums.cjs", () => {
	const { binaryNames, binSums, writeBinSums } = createRequire(import.meta.url)(
		"../scripts/bin-sums.cjs",
	);
	it("names the platform's executables", () => {
		expect(binaryNames("win32")).toEqual(["loams-agentd.exe", "loams.exe"]);
		expect(binaryNames("darwin")).toEqual(["loams-agentd", "loams"]);
		expect(binaryNames("linux")).toEqual(["loams-agentd", "loams"]);
	});
	it("lists both binaries in sha256sum format, ignoring other files", () => {
		const d = fresh("sums-both");
		writeFileSync(join(d, "loams-agentd.exe"), "daemon");
		writeFileSync(join(d, "loams.exe"), "engine");
		writeFileSync(join(d, "VERSION"), "0.1.0\n");
		expect(writeBinSums(d, "win32")).toBe(
			`${sha("daemon")}  loams-agentd.exe\n${sha("engine")}  loams.exe\n`,
		);
		expect(readFileSync(join(d, "SHA256SUMS"), "utf8")).toBe(binSums(d, "win32"));
	});
	it("lists the daemon alone when the engine is not bundled (a Windows build without it)", () => {
		const d = fresh("sums-daemon-only");
		writeFileSync(join(d, "loams-agentd.exe"), "daemon");
		expect(binSums(d, "win32")).toBe(`${sha("daemon")}  loams-agentd.exe\n`);
	});
	it("writes nothing without the daemon, and the platform picks the names", () => {
		const d = fresh("sums-none");
		writeFileSync(join(d, "loams-agentd"), "unix daemon");
		expect(writeBinSums(d, "win32")).toBeNull();
		expect(existsSync(join(d, "SHA256SUMS"))).toBe(false);
		expect(binSums(d, "darwin")).toBe(`${sha("unix daemon")}  loams-agentd\n`);
	});
	it("cli fails without the daemon", () => {
		const d = fresh("sums-cli");
		const r = spawnSync(process.execPath, [join(scripts, "bin-sums.cjs"), d, "linux"], {
			encoding: "utf8",
		});
		expect(r.status).toBe(1);
		expect(r.stderr).toMatch(/no loams-agentd/);
	});
});

// The macOS signer: SHA256SUMS must describe the binaries as signed, and the bundle is resealed after.
describe("mac-sign.cjs", () => {
	const { macSign, signWith, resealOptions, binDir } = createRequire(import.meta.url)(
		"../scripts/mac-sign.cjs",
	);
	const app = () => {
		const a = join(fresh("mac-sign"), "Loams Desktop.app");
		mkdirSync(binDir(a), { recursive: true });
		return a;
	};
	it("rewrites SHA256SUMS from the signed binaries, then reseals only the bundle", async () => {
		const a = app();
		const bin = binDir(a);
		writeFileSync(join(bin, "loams-agentd"), "daemon");
		writeFileSync(join(bin, "loams"), "engine");
		writeFileSync(join(bin, "SHA256SUMS"), "stale\n");
		const calls: { ignore?: (f: string) => boolean; app: string; identity: string }[] = [];
		// A signer that changes every binary, as codesign does.
		const sign = async (o: { ignore?: (f: string) => boolean; app: string; identity: string }) => {
			calls.push(o);
			if (calls.length === 1) {
				for (const f of ["loams-agentd", "loams"]) writeFileSync(join(bin, f), `${f} signed`);
			}
		};
		const opts = { app: a, identity: "-", ignore: () => false, binaries: ["x"] };
		await signWith(opts, sign);
		expect(calls).toHaveLength(2);
		expect(calls[0]).toBe(opts);
		expect(readFileSync(join(bin, "SHA256SUMS"), "utf8")).toBe(
			`${sha("loams signed")}  loams\n${sha("loams-agentd signed")}  loams-agentd\n`,
		);
		const reseal = calls[1];
		if (!reseal) throw new Error("no second signing pass");
		expect(reseal.identity).toBe("-");
		expect(reseal.ignore?.(a)).toBe(false);
		expect(reseal.ignore?.(join(bin, "loams-agentd"))).toBe(true);
		expect(reseal.ignore?.(join(a, "Contents", "MacOS", "Loams Desktop"))).toBe(true);
	});
	it("signs once when no daemon is bundled", async () => {
		const a = app();
		let n = 0;
		await signWith({ app: a, identity: "-" }, async () => void n++);
		expect(n).toBe(1);
		expect(existsSync(join(binDir(a), "SHA256SUMS"))).toBe(false);
	});
	it("the hook takes only the options: electron-builder passes its packager second", () => {
		// Desktop run 39092978401: a second `deps` parameter received the MacPackager, whose
		// unbound `sign` method then failed.
		expect(macSign.length).toBe(1);
	});
	it("reseal options keep the identity and keychain", () => {
		const o = resealOptions({ app: "/A.app", identity: "ABC", keychain: "/k", binaries: ["/b"] });
		expect(o).toMatchObject({ app: "/A.app", identity: "ABC", keychain: "/k", binaries: undefined });
	});
});

// gpg-sign.sh runs on the Linux release runner (desktop-electron-release.yml); on Windows, bash and
// gpg (when present at all) are Git for Windows' MSYS builds, which this bash script does not target.
const hasGpg =
	process.platform !== "win32" &&
	spawnSync("gpg", ["--version"]).status === 0;
describe.skipIf(!hasGpg)("gpg-sign.sh dry run with a throwaway key", () => {
	const run = (dir: string, env: Record<string, string>) =>
		spawnSync("bash", [join(scripts, "gpg-sign.sh"), dir], {
			env: { PATH: process.env.PATH ?? "", TMPDIR: scratch, ...env },
			encoding: "utf8",
		});

	it("skips with a warning and writes nothing when no key is set", () => {
		const d = fresh("gpg-nokey");
		writeFileSync(join(d, "a.deb"), "d");
		writeFileSync(join(d, "SHA256SUMS"), "s");
		const r = run(d, {});
		expect(r.status).toBe(0);
		expect(r.stderr).toContain("unsigned");
		expect(existsSync(join(d, "a.deb.sig"))).toBe(false);
		expect(existsSync(join(d, "SHA256SUMS.asc"))).toBe(false);
	});

	it("signs artifacts and SHA256SUMS; gpg --verify passes", () => {
		const gen = fresh("gpg-gen");
		const genEnv = { ...process.env, GNUPGHOME: gen };
		chmodOnly(gen);
		const uid = `loams-test-${randomBytes(3).toString("hex")}@example.invalid`;
		execFileSync(
			"gpg",
			[
				"--batch",
				"--passphrase",
				"",
				"--quick-generate-key",
				uid,
				"ed25519",
				"sign",
				"1d",
			],
			{ env: genEnv, stdio: "ignore" },
		);
		const secret = execFileSync(
			"gpg",
			["--armor", "--export-secret-keys", uid],
			{
				env: genEnv,
			},
		).toString();

		const d = fresh("gpg-signed");
		for (const n of [
			"a.AppImage",
			"b.deb",
			"c.pacman",
			"d.dmg",
			"e.rpm",
			"f.exe",
		])
			writeFileSync(join(d, n), n);
		execFileSync("node", [join(scripts, "checksums.mjs"), d]);

		const r = run(d, { LOAMS_GPG_PRIVATE_KEY: secret, LOAMS_GPG_KEY_ID: uid });
		expect(r.status, r.stderr).toBe(0);
		// rpm and exe are SignPath's, not GPG's.
		expect(existsSync(join(d, "e.rpm.sig"))).toBe(false);
		expect(existsSync(join(d, "f.exe.sig"))).toBe(false);
		for (const f of ["a.AppImage", "b.deb", "c.pacman", "d.dmg"])
			expect(existsSync(join(d, `${f}.sig`)), f).toBe(true);

		// Verify with a keyring holding only the public key.
		const pub = fresh("gpg-pub");
		chmodOnly(pub);
		const pubEnv = { ...process.env, GNUPGHOME: pub };
		const pubKey = execFileSync("gpg", ["--armor", "--export", uid], {
			env: genEnv,
		});
		execFileSync("gpg", ["--batch", "--import"], {
			env: pubEnv,
			input: pubKey,
			stdio: ["pipe", "ignore", "ignore"],
		});
		for (const [sig, file] of [
			["SHA256SUMS.asc", "SHA256SUMS"],
			["b.deb.sig", "b.deb"],
			["a.AppImage.sig", "a.AppImage"],
		]) {
			const v = spawnSync(
				"gpg",
				[
					"--batch",
					"--verify",
					join(d, sig as string),
					join(d, file as string),
				],
				{ env: pubEnv, encoding: "utf8" },
			);
			expect(v.status, v.stderr).toBe(0);
			expect(v.stderr).toContain("Good signature");
		}
		// A tampered artifact must not verify.
		writeFileSync(join(d, "b.deb"), "tampered");
		const bad = spawnSync(
			"gpg",
			["--batch", "--verify", join(d, "b.deb.sig"), join(d, "b.deb")],
			{ env: pubEnv },
		);
		expect(bad.status).not.toBe(0);
	}, 60_000); // key generation and gpg-agent start-up are slow on macOS runners

	it("fails clearly when the key cannot be imported", () => {
		const d = fresh("gpg-badkey");
		writeFileSync(join(d, "SHA256SUMS"), "s");
		const r = run(d, { LOAMS_GPG_PRIVATE_KEY: "not a key" });
		expect(r.status).toBe(1);
		expect(r.stderr).toContain("could not be imported");
	});

	it("fails when a key is set but SHA256SUMS is missing", () => {
		const d = fresh("gpg-nosums");
		const r = run(d, { LOAMS_GPG_PRIVATE_KEY: "x" });
		expect(r.status).toBe(1);
	});
});

function chmodOnly(dir: string) {
	execFileSync("chmod", ["700", dir]);
}
