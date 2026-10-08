// JSON Schema (2020-12) validation of a connector instance config. Runs in the main process
// (the console's CSP forbids the `new Function` that ajv compiles with) and in the browser
// preview's fake desktop, where no CSP applies.
import Ajv2020 from "ajv/dist/2020.js";
import type { ConnectorValidation } from "./contracts";

// `x-loams-*` annotations and unknown formats are not errors for this tool.
const ajv = new Ajv2020({ allErrors: true, strict: false, validateFormats: false });
const cache = new WeakMap<object, ReturnType<typeof ajv.compile>>();

export function validateConfig(
	schema: Record<string, unknown>,
	config: unknown,
): ConnectorValidation {
	let fn = cache.get(schema);
	if (!fn) {
		// Several schemas share nothing but each carries an `$id`; drop it so recompiles never collide.
		const { $id: _id, ...rest } = schema;
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
