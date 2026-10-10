#!/usr/bin/env node
// Usage: node checksums.mjs <dir> [--out SHA256SUMS]
// Writes a `sha256sum -c` compatible file covering every release artifact directly inside <dir>.
// Signatures and the checksum file itself are excluded (they are produced after, and cover this file).
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const EXCLUDED = /(^SHA256SUMS(\.asc)?$)|(\.(sig|asc)$)/;

/** @param {string} dir @returns {string[]} sorted artifact file names */
export function artifactNames(dir) {
	return readdirSync(dir)
		.filter((n) => !EXCLUDED.test(n) && statSync(join(dir, n)).isFile())
		.sort();
}

/** @param {string} dir @returns {string} the SHA256SUMS text: `<hex>  <name>` per line, sorted by name */
export function sha256sums(dir) {
	return artifactNames(dir)
		.map(
			(n) =>
				`${createHash("sha256")
					.update(readFileSync(join(dir, n)))
					.digest("hex")}  ${n}\n`,
		)
		.join("");
}

if (
	process.argv[1] &&
	resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const args = process.argv.slice(2);
	const dir = args[0];
	const outIdx = args.indexOf("--out");
	if (!dir) {
		console.error("usage: checksums.mjs <dir> [--out SHA256SUMS]");
		process.exit(2);
	}
	const out = join(
		dir,
		outIdx >= 0 && args[outIdx + 1] ? basename(args[outIdx + 1]) : "SHA256SUMS",
	);
	const text = sha256sums(dir);
	if (!text) {
		console.error(
			`no artifacts in ${dir}; refusing to write an empty SHA256SUMS`,
		);
		process.exit(1);
	}
	writeFileSync(out, text);
	console.log(`wrote ${out} (${text.trim().split("\n").length} files)`);
}
