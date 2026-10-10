// Builds resources/connectors.json from connectors/registry/*.yaml and
// connectors/schemas/*.config.json (plan AP1e Task 27, D673). The Connectors page reads it
// through the main process (`connectors.catalog()` / `connectors.get(id)`); electron-builder
// ships it as an extraResource.
//
// Usage: node scripts/connectors-catalog.mjs [outFile]
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

const here = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(here, "..", "..", "..");

/** The capability flags that name a way of moving data. */
const MODES = ["streaming", "batch", "cdc", "webhook"];

/** The enabled modes of one side (`capabilities.source` or `.sink`); null when it has no such side. */
function sideModes(side) {
	if (!side || typeof side !== "object") return null;
	return MODES.filter((m) => side[m] === true);
}

/**
 * A schema is a stub when it declares no `properties`, or carries gen_registry.py's
 * `x-loams-generated-by` tag (written on the placeholder for every row without a hand-written manifest).
 */
export function isStubSchema(schema) {
	if (schema && typeof schema === "object" && "x-loams-generated-by" in schema)
		return true;
	const props = schema?.properties;
	return !props || typeof props !== "object" || Object.keys(props).length === 0;
}

export function summarise(manifest, schema) {
	const source = sideModes(manifest.capabilities?.source);
	const sink = sideModes(manifest.capabilities?.sink);
	return {
		id: manifest.id,
		name: manifest.name,
		category: manifest.category,
		status: manifest.status,
		runtime: {
			kind: manifest.runtime?.kind ?? "",
			ref: manifest.runtime?.ref ?? "",
		},
		source,
		sink,
		modes: [...new Set([...(source ?? []), ...(sink ?? [])])],
		auth: Array.isArray(manifest.auth) ? manifest.auth : [],
		licence: manifest.licence?.component ?? "",
		stub: isStubSchema(schema),
	};
}

export function buildCatalog(repoRoot = REPO_ROOT) {
	const regDir = join(repoRoot, "connectors", "registry");
	const schemaDir = join(repoRoot, "connectors", "schemas");
	const connectors = [];
	const details = {};
	for (const f of readdirSync(regDir)
		.filter((n) => n.endsWith(".yaml"))
		.sort()) {
		const manifest = parse(readFileSync(join(regDir, f), "utf8"));
		if (!manifest || typeof manifest.id !== "string") {
			throw new Error(`${f}: not a connector manifest`);
		}
		const schema = JSON.parse(
			readFileSync(join(schemaDir, `${manifest.id}.config.json`), "utf8"),
		);
		connectors.push(summarise(manifest, schema));
		details[manifest.id] = { manifest, schema };
	}
	connectors.sort((a, b) => a.id.localeCompare(b.id));
	return { version: 1, connectors, details };
}

if (
	process.argv[1] &&
	resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const out = resolve(
		process.argv[2] ?? join(here, "..", "resources", "connectors.json"),
	);
	const catalog = buildCatalog();
	mkdirSync(dirname(out), { recursive: true });
	writeFileSync(out, `${JSON.stringify(catalog)}\n`);
	const stubs = catalog.connectors.filter((c) => c.stub).length;
	console.log(
		`connectors catalog: ${catalog.connectors.length} connectors (${stubs} stubs) -> ${out}`,
	);
}
