#!/usr/bin/env node
// Usage: node refresh-manifest.mjs <latest*.yml> <dir>
// SignPath replaces the Windows installer after electron-builder wrote latest.yml, so the manifest's
// sha512 and size are stale. Recompute them from the files now in <dir>, and delete the matching
// .blockmap (also stale; electron-updater falls back to a full download when it is absent).
import { createHash } from "node:crypto";
import {
	existsSync,
	readFileSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import yaml from "js-yaml";

/** @param {string} ymlPath @param {string} dir @returns {string[]} names refreshed */
export function refreshManifest(ymlPath, dir) {
	const doc = yaml.load(readFileSync(ymlPath, "utf8"));
	const refreshed = [];
	const digest = (name) => {
		const p = join(dir, name);
		if (!existsSync(p))
			throw new Error(`${ymlPath} lists ${name}, which is not in ${dir}`);
		const buf = readFileSync(p);
		rmSync(`${p}.blockmap`, { force: true });
		return {
			sha512: createHash("sha512").update(buf).digest("base64"),
			size: statSync(p).size,
		};
	};
	for (const f of doc.files ?? []) {
		Object.assign(f, digest(f.url));
		delete f.blockMapSize;
		refreshed.push(f.url);
	}
	if (doc.path) doc.sha512 = digest(doc.path).sha512;
	writeFileSync(ymlPath, yaml.dump(doc, { lineWidth: -1 }));
	return refreshed;
}

if (
	process.argv[1] &&
	resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const [yml, dir] = process.argv.slice(2);
	if (!yml || !dir) {
		console.error("usage: refresh-manifest.mjs <latest*.yml> <dir>");
		process.exit(2);
	}
	console.log(`refreshed ${refreshManifest(yml, dir).join(", ")}`);
}
