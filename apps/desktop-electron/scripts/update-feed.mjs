#!/usr/bin/env node
// The desktop update feed (`desktop-latest`): key checks and feed staging, shared by
// desktop-electron-release.yml (signing) and desktop-promote.yml (promotion). No dependencies, so
// the promote job runs it with a bare Node and no install.
//
//   update-feed.mjs pinned                 print the pinned public key (update-pubkey.txt), or nothing
//   update-feed.mjs check-key              LOAMS_UPDATE_SIGNING_KEY's public half must equal the pinned key
//   update-feed.mjs payloads <dir>         the payload files the latest*.yml in <dir> name, one per line
//   update-feed.mjs stage <src> <out>      verify every latest*.yml in <src> (signature against the pinned
//                                          key, payloads present with the listed sha512 and size) and copy
//                                          manifests, signatures and payloads to <out>
//
// LOAMS_UPDATE_PUBKEY_FILE overrides the pinned key file (tests only).
import { createHash, createPrivateKey, createPublicKey, verify } from "node:crypto";
import {
	copyFileSync,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	statSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HEX32 = /^[0-9a-f]{64}$/;
const here = dirname(fileURLToPath(import.meta.url));
export const PINNED_FILE = resolve(here, "..", "update-pubkey.txt");

/** The pinned key: the first non-comment line of update-pubkey.txt, lowercased; "" when unset. */
export function readPinned(file = process.env.LOAMS_UPDATE_PUBKEY_FILE || PINNED_FILE) {
	if (!existsSync(file)) return "";
	const line = readFileSync(file, "utf8")
		.split(/\r?\n/)
		.map((l) => l.trim())
		.find((l) => l && !l.startsWith("#"));
	if (!line) return "";
	const k = line.toLowerCase();
	if (!HEX32.test(k)) throw new Error(`${file}: not 64 hex characters`);
	return k;
}

/** Ed25519 public key (hex) of a 32-byte seed (hex). */
export function pubkeyOfSeed(seedHex) {
	if (!HEX32.test(String(seedHex).toLowerCase())) throw new Error("the seed is not 64 hex characters");
	const der = Buffer.concat([
		Buffer.from("302e020100300506032b657004220420", "hex"),
		Buffer.from(seedHex, "hex"),
	]);
	const pub = createPublicKey(createPrivateKey({ key: der, format: "der", type: "pkcs8" }));
	return pub.export({ format: "der", type: "spki" }).subarray(-32).toString("hex");
}

/** True when `sigB64` is a valid Ed25519 signature of `data` under `pubHex` (what the app checks). */
export function verifySig(data, sigB64, pubHex) {
	try {
		const sig = Buffer.from(String(sigB64).trim(), "base64");
		if (sig.length !== 64 || !HEX32.test(pubHex)) return false;
		const key = createPublicKey({
			key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(pubHex, "hex")]),
			format: "der",
			type: "spki",
		});
		return verify(null, data, key, sig);
	} catch {
		return false;
	}
}

const unquote = (v) => v.trim().replace(/^(['"])(.*)\1$/, "$2");

/**
 * The electron-builder manifest shape, read without a YAML library:
 * `version`, `path`, `sha512` at the top, and `files: [{url, sha512, size}]`.
 */
export function parseManifest(text) {
	const out = { version: "", files: [], path: "", sha512: "" };
	let inFiles = false;
	let cur = null;
	for (const raw of text.split(/\r?\n/)) {
		if (!raw.trim() || raw.trim().startsWith("#")) continue;
		const top = /^([A-Za-z0-9_]+):\s*(.*)$/.exec(raw);
		if (top) {
			inFiles = top[1] === "files";
			cur = null;
			if (top[1] === "version") out.version = unquote(top[2]);
			if (top[1] === "path") out.path = unquote(top[2]);
			if (top[1] === "sha512") out.sha512 = unquote(top[2]);
			continue;
		}
		if (!inFiles) continue;
		const item = /^\s*(-\s+)?([A-Za-z0-9_]+):\s*(.*)$/.exec(raw);
		if (!item) continue;
		if (item[1]) {
			cur = {};
			out.files.push(cur);
		}
		if (!cur) continue;
		const v = unquote(item[3]);
		if (item[2] === "size") cur.size = Number(v);
		else cur[item[2]] = v;
	}
	return out;
}

/** A file name a manifest may refer to: a plain name, never a path or URL. */
const safeName = (n) => /^[A-Za-z0-9][A-Za-z0-9._+-]*$/.test(n);

export const manifestsIn = (dir) =>
	readdirSync(dir)
		.filter((n) => /^latest.*\.yml$/.test(n))
		.sort();

/** Payload names the manifests in `dir` reference (files[].url and path), deduplicated, sorted. */
export function payloadNames(dir) {
	const names = new Set();
	for (const m of manifestsIn(dir)) {
		const doc = parseManifest(readFileSync(join(dir, m), "utf8"));
		for (const f of doc.files) if (f.url) names.add(f.url);
		if (doc.path) names.add(doc.path);
	}
	for (const n of names) if (!safeName(n)) throw new Error(`a manifest names "${n}", which is not a plain file name`);
	return [...names].sort();
}

const sha512b64 = (p) => createHash("sha512").update(readFileSync(p)).digest("base64");

/**
 * Verify the feed in `src` and copy it to `out`. Throws, copying nothing, on: no manifest, a manifest
 * without a `.sig` or with a signature the pinned key does not verify, a missing version, a payload
 * that is absent or whose sha512/size differs from the manifest, or manifests of different versions.
 * Returns { version, manifests, payloads }.
 */
export function stageFeed(src, out, pubHex) {
	if (!HEX32.test(pubHex ?? "")) throw new Error("no pinned update public key (update-pubkey.txt is empty)");
	const manifests = manifestsIn(src);
	if (manifests.length === 0) throw new Error(`${src} has no latest*.yml`);
	const versions = new Set();
	for (const m of manifests) {
		const p = join(src, m);
		if (!existsSync(`${p}.sig`)) throw new Error(`${m} has no ${m}.sig; the feed never serves an unsigned manifest`);
		const bytes = readFileSync(p);
		if (!verifySig(bytes, readFileSync(`${p}.sig`, "utf8"), pubHex))
			throw new Error(`${m}.sig does not verify against the pinned key`);
		const doc = parseManifest(bytes.toString("utf8"));
		if (!doc.version) throw new Error(`${m} has no version`);
		if (doc.files.length === 0) throw new Error(`${m} lists no files`);
		versions.add(doc.version);
		for (const f of doc.files) {
			if (!f.url || !f.sha512) throw new Error(`${m}: a file entry lacks url or sha512`);
			if (!safeName(f.url)) throw new Error(`${m}: "${f.url}" is not a plain file name`);
			const fp = join(src, f.url);
			if (!existsSync(fp)) throw new Error(`${m} names ${f.url}, which is not in ${src}`);
			if (sha512b64(fp) !== f.sha512) throw new Error(`${f.url}: sha512 differs from ${m}`);
			if (f.size !== undefined && statSync(fp).size !== f.size) throw new Error(`${f.url}: size differs from ${m}`);
		}
		if (doc.path) {
			const entry = doc.files.find((f) => f.url === doc.path);
			if (!entry) throw new Error(`${m}: path ${doc.path} is not among its files`);
			if (doc.sha512 && doc.sha512 !== entry.sha512) throw new Error(`${m}: top-level sha512 differs from ${doc.path}'s`);
		}
	}
	if (versions.size !== 1) throw new Error(`the manifests disagree on the version: ${[...versions].join(", ")}`);
	const payloads = payloadNames(src);
	mkdirSync(out, { recursive: true });
	for (const n of payloads) copyFileSync(join(src, n), join(out, n));
	for (const m of manifests) {
		copyFileSync(join(src, `${m}.sig`), join(out, `${m}.sig`));
		copyFileSync(join(src, m), join(out, m));
	}
	return { version: [...versions][0], manifests, payloads };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	const [cmd, a, b] = process.argv.slice(2);
	try {
		if (cmd === "pinned") {
			const k = readPinned();
			if (k) console.log(k);
		} else if (cmd === "check-key") {
			const pinned = readPinned();
			if (!pinned) throw new Error("update-pubkey.txt is empty: commit the public key first");
			const seed = (process.env.LOAMS_UPDATE_SIGNING_KEY ?? "").trim().toLowerCase();
			if (pubkeyOfSeed(seed) !== pinned)
				throw new Error("LOAMS_UPDATE_SIGNING_KEY is not the private half of the pinned update-pubkey.txt");
			console.log("the signing key matches the pinned public key");
		} else if (cmd === "payloads" && a) {
			for (const n of payloadNames(a)) console.log(n);
		} else if (cmd === "stage" && a && b) {
			const r = stageFeed(a, b, readPinned());
			console.log(`staged ${r.version}: ${r.manifests.join(", ")}; payloads ${r.payloads.join(", ")}`);
		} else {
			console.error("usage: update-feed.mjs pinned | check-key | payloads <dir> | stage <src> <out>");
			process.exit(2);
		}
	} catch (e) {
		console.error(`::error::${e instanceof Error ? e.message : e}`);
		process.exit(1);
	}
}
