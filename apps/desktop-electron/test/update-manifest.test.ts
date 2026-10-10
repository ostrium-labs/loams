import { randomBytes } from "node:crypto";
import * as ed from "@noble/ed25519";
import { describe, expect, it } from "vitest";
import { verifyManifest } from "../src/main/update/manifest";
import {
	channelFile,
	compareVersions,
	decideUpdateMode,
	firstCheckDelayMs,
	parseManifestVersion,
	releaseUrl,
} from "../src/main/update/policy";

const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const yml = new TextEncoder().encode(
	"version: 1.2.3\nfiles:\n  - url: Loams-1.2.3.AppImage\n    sha512: abc\n",
);

async function keypair() {
	const priv = randomBytes(32);
	const pub = await ed.getPublicKeyAsync(priv);
	return { priv, pubHex: hex(pub) };
}
async function sign(priv: Uint8Array, data: Uint8Array) {
	return Buffer.from(await ed.signAsync(data, priv)).toString("base64");
}

describe("verifyManifest", () => {
	it("valid_signature_accepted", async () => {
		const k = await keypair();
		expect(await verifyManifest(yml, await sign(k.priv, yml), k.pubHex)).toBe(
			true,
		);
	});
	it("unsigned_manifest_is_refused", async () => {
		const k = await keypair();
		expect(await verifyManifest(yml, "", k.pubHex)).toBe(false);
		expect(await verifyManifest(yml, "not base64 !!", k.pubHex)).toBe(false);
		expect(await verifyManifest(yml, "AAAA", k.pubHex)).toBe(false);
	});
	it("manifest_for_other_key_is_refused", async () => {
		const k = await keypair();
		const other = await keypair();
		expect(
			await verifyManifest(yml, await sign(other.priv, yml), k.pubHex),
		).toBe(false);
	});
	it("tampered_manifest_refused", async () => {
		const k = await keypair();
		const sig = await sign(k.priv, yml);
		const bad = new TextEncoder().encode(
			new TextDecoder().decode(yml).replace("abc", "abd"),
		);
		expect(await verifyManifest(bad, sig, k.pubHex)).toBe(false);
	});
	it("refuses_bad_pubkey", async () => {
		const k = await keypair();
		expect(await verifyManifest(yml, await sign(k.priv, yml), "")).toBe(false);
		expect(await verifyManifest(yml, await sign(k.priv, yml), "zz")).toBe(
			false,
		);
	});
});

const base = {
	packaged: true,
	feed: "https://updates.example/loams",
	pubkeyHex: "ab".repeat(32),
	platform: "linux" as NodeJS.Platform,
	appImage: "/home/u/Loams.AppImage" as string | undefined,
};

describe("decideUpdateMode", () => {
	it("disabled_without_feed_or_key", () => {
		expect(decideUpdateMode({ ...base, feed: "" })).toBe("disabled");
		expect(decideUpdateMode({ ...base, pubkeyHex: "" })).toBe("disabled");
		expect(decideUpdateMode({ ...base, packaged: false })).toBe("disabled");
		expect(decideUpdateMode({ ...base, feed: "file:///x" })).toBe("disabled");
		expect(decideUpdateMode({ ...base, pubkeyHex: "abc" })).toBe("disabled");
	});
	it("macos_is_manual_download", () => {
		expect(decideUpdateMode({ ...base, platform: "darwin" })).toBe("manual");
	});
	it("windows_self_updates", () => {
		expect(decideUpdateMode({ ...base, platform: "win32" })).toBe("self");
	});
	it("linux_appimage_self_updates_package_installs_are_manual", () => {
		expect(decideUpdateMode(base)).toBe("self");
		expect(decideUpdateMode({ ...base, appImage: undefined })).toBe("manual");
		expect(decideUpdateMode({ ...base, appImage: "" })).toBe("manual");
	});
});

describe("helpers", () => {
	it("channel_files", () => {
		expect(channelFile("win32", "x64")).toBe("latest.yml");
		expect(channelFile("darwin", "arm64")).toBe("latest-mac.yml");
		expect(channelFile("linux", "x64")).toBe("latest-linux.yml");
		expect(channelFile("linux", "arm64")).toBe("latest-linux-arm64.yml");
	});
	it("versions", () => {
		expect(parseManifestVersion("version: 1.2.3\nfiles: []")).toBe("1.2.3");
		expect(parseManifestVersion("version: '2.0.0-beta.1'\n")).toBe(
			"2.0.0-beta.1",
		);
		expect(parseManifestVersion("files: []")).toBeNull();
		expect(compareVersions("1.2.4", "1.2.3")).toBeGreaterThan(0);
		expect(compareVersions("1.2.3", "1.2.3")).toBe(0);
		expect(compareVersions("1.10.0", "1.9.9")).toBeGreaterThan(0);
		expect(compareVersions("1.0.0-beta.1", "1.0.0")).toBeLessThan(0);
	});
	it("first_check_is_15s_plus_up_to_30s_jitter", () => {
		expect(firstCheckDelayMs(0)).toBe(15_000);
		expect(firstCheckDelayMs(0.999999)).toBeLessThan(45_000);
	});
	it("release_url_is_https", () => {
		expect(releaseUrl("1.2.3")).toMatch(/^https:\/\/.*1\.2\.3$/);
	});
});

describe("sign-manifest.mjs", () => {
	it("produces_a_signature_verifyManifest_accepts", async () => {
		const { mkdtempSync, writeFileSync, readFileSync } = await import(
			"node:fs"
		);
		const { tmpdir } = await import("node:os");
		const { join } = await import("node:path");
		const { execFileSync } = await import("node:child_process");
		const k = await keypair();
		const dir = mkdtempSync(join(tmpdir(), "sign-"));
		const f = join(dir, "latest.yml");
		writeFileSync(f, yml);
		execFileSync("node", [join(__dirname, "../scripts/sign-manifest.mjs"), f], {
			env: { ...process.env, LOAMS_UPDATE_SIGNING_KEY: hex(k.priv) },
		});
		expect(
			await verifyManifest(yml, readFileSync(`${f}.sig`, "utf8"), k.pubHex),
		).toBe(true);
	});
});
