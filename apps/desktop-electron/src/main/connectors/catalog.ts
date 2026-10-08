import { join } from "node:path";
import type {
	ConnectorDetail,
	ConnectorSummary,
	ConnectorValidation,
	IpcResult,
} from "../../shared/contracts";
import { validateConfig } from "../../shared/validate";

interface CatalogFile {
	version: number;
	connectors: ConnectorSummary[];
	details: Record<string, ConnectorDetail>;
}

/** Where connectors.json lives: an extraResource when packaged, resources/ in the app dir in dev. */
export function catalogPath(env: {
	isPackaged: boolean;
	resourcesPath: string;
	appRoot: string;
}): string {
	return env.isPackaged
		? join(env.resourcesPath, "connectors.json")
		: join(env.appRoot, "resources", "connectors.json");
}

export const MAX_CONFIG_BYTES = 256 * 1024;
export const MAX_CONFIG_DEPTH = 32;

function depthOf(v: unknown, d = 1): number {
	if (typeof v !== "object" || v === null) return d;
	let max = d;
	for (const c of Object.values(v)) {
		max = Math.max(max, depthOf(c, d + 1));
		if (max > MAX_CONFIG_DEPTH) break;
	}
	return max;
}

/** An error message when `config` is not a plain object, or is too big or too deep; else undefined. */
export function checkConfig(config: unknown): string | undefined {
	if (typeof config !== "object" || config === null || Array.isArray(config))
		return "The config must be an object.";
	const proto = Object.getPrototypeOf(config);
	if (proto !== Object.prototype && proto !== null)
		return "The config must be a plain object.";
	if (depthOf(config) > MAX_CONFIG_DEPTH)
		return `The config is nested deeper than ${MAX_CONFIG_DEPTH} levels.`;
	let json: string | undefined;
	try {
		json = JSON.stringify(config);
	} catch {
		return "The config is not JSON.";
	}
	if (json === undefined || json.length > MAX_CONFIG_BYTES)
		return "The config is larger than 256 KiB.";
	return undefined;
}

/** The connector catalog, read once from the generated JSON. */
export class ConnectorCatalog {
	private data?: CatalogFile;

	constructor(private readonly readFile: () => string) {}

	private load(): CatalogFile {
		if (!this.data) {
			const parsed = JSON.parse(this.readFile()) as CatalogFile;
			if (
				!Array.isArray(parsed.connectors) ||
				typeof parsed.details !== "object"
			)
				throw new Error("connectors.json has an unexpected shape");
			this.data = parsed;
		}
		return this.data;
	}

	catalog(): ConnectorSummary[] {
		return this.load().connectors;
	}

	get(id: unknown): IpcResult<ConnectorDetail> {
		if (typeof id !== "string")
			return { ok: false, code: "bad_request", message: "Invalid request" };
		const d = Object.hasOwn(this.load().details, id)
			? this.load().details[id]
			: undefined;
		return d
			? { ok: true, value: d }
			: { ok: false, code: "not_found", message: `No connector "${id}".` };
	}

	validate(id: unknown, config: unknown): IpcResult<ConnectorValidation> {
		const d = this.get(id);
		if (!d.ok) return d;
		const bad = checkConfig(config);
		if (bad) return { ok: false, code: "bad_request", message: bad };
		const s = d.value.manifest.secrets;
		const secrets = Array.isArray(s)
			? s.filter((x): x is string => typeof x === "string")
			: [];
		return {
			ok: true,
			value: validateConfig(d.value.schema, config, secrets),
		};
	}
}
