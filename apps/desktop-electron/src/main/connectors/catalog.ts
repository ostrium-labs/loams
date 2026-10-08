import { join } from "node:path";
import { validateConfig } from "../../shared/validate";
import type {
	ConnectorDetail,
	ConnectorSummary,
	ConnectorValidation,
	IpcResult,
} from "../../shared/contracts";

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
		return { ok: true, value: validateConfig(d.value.schema, config) };
	}
}
