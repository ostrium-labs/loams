// The marketplace schemas, read from the repository's schemas/marketplace (the
// same files agentd compiles in), so the CLI and agentd never disagree.
import { readFileSync } from 'node:fs';
import { Ajv2020, type ErrorObject, type ValidateFunction } from 'ajv/dist/2020.js';

const SCHEMAS = new URL('../../../schemas/marketplace/', import.meta.url);

export type SchemaName = 'listing.v1.json' | 'index.v1.json' | 'revocations.v1.json';

export function readSchema(name: SchemaName): Record<string, unknown> {
  return JSON.parse(readFileSync(new URL(name, SCHEMAS), 'utf8')) as Record<string, unknown>;
}

const NAMES: SchemaName[] = ['listing.v1.json', 'index.v1.json', 'revocations.v1.json'];
let ajv: Ajv2020 | undefined;
const cache = new Map<SchemaName, ValidateFunction>();

function compiled(name: SchemaName): ValidateFunction {
  let validate = cache.get(name);
  if (!validate) {
    if (!ajv) {
      // strict: false because the schemas carry x-proto-* annotations. All three are
      // registered, as index.v1.json refers to the listing schema by its $id.
      ajv = new Ajv2020({ allErrors: true, strict: false });
      for (const each of NAMES) ajv.addSchema(readSchema(each), each);
    }
    validate = ajv.getSchema(name);
    if (!validate) throw new Error(`schema ${name} did not compile`);
    cache.set(name, validate);
  }
  return validate;
}

export interface SchemaIssue {
  /** JSON Pointer to the offending value ('' for the document). */
  path: string;
  message: string;
}

function describe(error: ErrorObject): SchemaIssue {
  const params = error.params as Record<string, unknown>;
  let message = error.message ?? error.keyword;
  if (error.keyword === 'additionalProperties') message += `: ${String(params.additionalProperty)}`;
  if (error.keyword === 'enum') message += `: ${JSON.stringify(params.allowedValues)}`;
  return { path: error.instancePath, message };
}

/** Schema issues, deduplicated and in a stable order. */
export function validateAgainst(name: SchemaName, doc: unknown): SchemaIssue[] {
  const validate = compiled(name);
  if (validate(doc)) return [];
  const seen = new Set<string>();
  const issues: SchemaIssue[] = [];
  for (const error of validate.errors ?? []) {
    // `if` failures only say that a `then` failed; the `then`'s own errors carry the detail.
    if (error.keyword === 'if') continue;
    const issue = describe(error);
    const key = `${issue.path}\u0000${issue.message}`;
    if (!seen.has(key)) {
      seen.add(key);
      issues.push(issue);
    }
  }
  if (issues.length === 0) issues.push({ path: '', message: 'does not match the schema' });
  return issues;
}
