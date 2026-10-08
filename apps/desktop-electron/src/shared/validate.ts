// JSON Schema (2020-12) validation of a connector instance config. Runs in the main process
// (the console's CSP forbids the `new Function` that ajv compiles with) and in the browser
// preview's fake desktop, where no CSP applies.
import Ajv2020 from "ajv/dist/2020.js";
import type { ConnectorValidation } from "./contracts";

// `x-loams-*` annotations and unknown formats are not errors for this tool.
const ajv = new Ajv2020({
	allErrors: true,
	strict: false,
	validateFormats: false,
});
const cache = new WeakMap<object, ReturnType<typeof ajv.compile>>();

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj =>
	typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * The schema with every secret field loosened to a plain string. A secret reaches validation as
 * the placeholder `${secret:<path>}`, so its minLength, pattern, format or enum must not apply;
 * presence (`required`) and the object shape are kept. A field is secret when it is `writeOnly`
 * or its dotted path is listed in the manifest's `secrets`.
 */
export function loosenSecrets(
	schema: Obj,
	secrets: string[],
	parent = "",
): Obj {
	if (!isObj(schema.properties)) return schema;
	const properties: Obj = {};
	for (const [key, raw] of Object.entries(schema.properties)) {
		const id = parent ? `${parent}.${key}` : key;
		if (isObj(raw) && (raw.writeOnly === true || secrets.includes(id))) {
			properties[key] = { type: "string", writeOnly: true };
		} else if (isObj(raw)) {
			properties[key] = loosenSecrets(raw, secrets, id);
		} else properties[key] = raw;
	}
	return { ...schema, properties };
}

export function validateConfig(
	schema: Obj,
	config: unknown,
	secrets: string[] = [],
): ConnectorValidation {
	let fn = cache.get(schema);
	if (!fn) {
		// Several schemas share nothing but each carries an `$id`; drop it so recompiles never collide.
		const { $id: _id, ...rest } = loosenSecrets(schema, secrets);
		fn = ajv.compile(rest);
		cache.set(schema, fn);
	}
	const valid = fn(config) as boolean;
	return {
		valid,
		errors: (fn.errors ?? []).map((e) => ({
			// A missing property is reported at the property, so a form can mark its field.
			path:
				e.keyword === "required"
					? `${e.instancePath}/${String((e.params as { missingProperty: string }).missingProperty)}`
					: e.instancePath,
			message: e.message ?? "invalid",
		})),
	};
}
