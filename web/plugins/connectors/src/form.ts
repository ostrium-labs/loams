// The schema-driven configure form, as pure functions: a JSON Schema (2020-12 subset: string,
// number, integer, boolean, enum, object, array of primitives) becomes a field tree; the form's
// string values become a config object; the config becomes instance YAML.
//
// Secrets: a field is secret when the schema says `writeOnly` or the manifest lists its dotted
// path under `secrets`. A secret value is typed into a password input, kept in component state
// only, and never reaches the config, the YAML or any storage: the config carries the
// placeholder `${secret:<path>}` instead.

import { stringify } from 'yaml';

export type FieldKind = 'string' | 'number' | 'integer' | 'boolean' | 'enum' | 'object' | 'array';
export type ItemKind = 'string' | 'number' | 'integer' | 'boolean';

export interface FieldDef {
  /** Dotted path, also the form value key and the secret name ("sasl.password"). */
  id: string;
  path: string[];
  key: string;
  kind: FieldKind;
  description?: string;
  required: boolean;
  secret: boolean;
  default?: unknown;
  enum?: unknown[];
  children?: FieldDef[];
  itemKind?: ItemKind;
}

type Schema = Record<string, unknown>;
const isObj = (v: unknown): v is Schema => typeof v === 'object' && v !== null && !Array.isArray(v);

export const secretPlaceholder = (id: string) => `\${secret:${id}}`;

function kindOf(s: Schema): FieldKind | undefined {
  if (Array.isArray(s.enum)) return 'enum';
  switch (s.type) {
    case 'string':
    case 'number':
    case 'integer':
    case 'boolean':
    case 'object':
    case 'array':
      return s.type;
    default:
      return undefined;
  }
}

function itemKindOf(s: Schema): ItemKind | undefined {
  const items = s.items;
  if (!isObj(items)) return 'string';
  const t = items.type;
  return t === 'string' || t === 'number' || t === 'integer' || t === 'boolean' ? t : undefined;
}

/** The fields an object schema declares, in declaration order. Unsupported shapes are skipped. */
export function buildFields(
  schema: Schema,
  secrets: string[] = [],
  parent: string[] = [],
): FieldDef[] {
  const props = isObj(schema.properties) ? schema.properties : {};
  const required = Array.isArray(schema.required) ? (schema.required as string[]) : [];
  const out: FieldDef[] = [];
  for (const [key, raw] of Object.entries(props)) {
    if (!isObj(raw)) continue;
    const kind = kindOf(raw);
    if (!kind) continue;
    const path = [...parent, key];
    const id = path.join('.');
    const def: FieldDef = {
      id,
      path,
      key,
      kind,
      description: typeof raw.description === 'string' ? raw.description : undefined,
      required: required.includes(key),
      secret: raw.writeOnly === true || secrets.includes(id),
      default: raw.default,
    };
    if (kind === 'enum') def.enum = raw.enum as unknown[];
    if (kind === 'object') {
      def.children = buildFields(raw, secrets, path);
      if (def.children.length === 0) continue;
    }
    if (kind === 'array') {
      const item = itemKindOf(raw);
      if (!item) continue;
      def.itemKind = item;
    }
    out.push(def);
  }
  return out;
}

/** Form values: one string per leaf field, keyed by dotted path. Empty means "not set". */
export type Values = Record<string, string>;

const flat = (fields: FieldDef[]): FieldDef[] =>
  fields.flatMap((f) => (f.children ? flat(f.children) : [f]));

export const leaves = flat;

function coerce(kind: FieldKind | ItemKind, raw: string, enumValues?: unknown[]): unknown {
  const v = raw.trim();
  switch (kind) {
    case 'number':
    case 'integer': {
      const n = Number(v);
      return Number.isNaN(n) ? v : n;
    }
    case 'boolean':
      return v === 'true' ? true : v === 'false' ? false : v;
    case 'enum':
      return enumValues?.find((e) => String(e) === v) ?? v;
    default:
      return raw;
  }
}

const splitItems = (raw: string) =>
  raw
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);

function build(fields: FieldDef[], values: Values): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of fields) {
    if (f.kind === 'object') {
      const child = build(f.children ?? [], values);
      if (Object.keys(child).length > 0) out[f.key] = child;
      continue;
    }
    const raw = values[f.id] ?? '';
    if (f.secret) {
      // Only that a value was entered matters; the value itself goes no further.
      if (raw !== '') out[f.key] = secretPlaceholder(f.id);
      continue;
    }
    if (f.kind === 'array') {
      const items = splitItems(raw);
      if (items.length > 0 && f.itemKind) {
        out[f.key] = items.map((i) => coerce(f.itemKind as ItemKind, i));
      }
      continue;
    }
    if (raw.trim() === '') {
      // A required field with a schema default takes it, so "required" is satisfiable.
      if (f.required && f.default !== undefined) out[f.key] = f.default;
      continue;
    }
    out[f.key] = coerce(f.kind, raw, f.enum);
  }
  return out;
}

/** The config object for the form's values. Secret fields hold `${secret:<path>}` placeholders. */
export function buildConfig(fields: FieldDef[], values: Values): Record<string, unknown> {
  return build(fields, values);
}

export interface InstanceDoc {
  connector: string;
  name: string;
  config: Record<string, unknown>;
}

/** The instance YAML. It is built from the placeholder config, so it cannot hold a secret value. */
export function exportYaml({ connector, name, config }: InstanceDoc): string {
  return stringify({
    apiVersion: 'loams.flow/v1',
    kind: 'ConnectorInstance',
    metadata: { name },
    spec: { connector, config },
  });
}

/** The form field id an ajv JSON Pointer belongs to (the nearest declared ancestor). */
export function fieldForPointer(fields: FieldDef[], pointer: string): string | undefined {
  const segs = pointer
    .split('/')
    .slice(1)
    .map((s) => s.replace(/~1/g, '/').replace(/~0/g, '~'));
  const ids = new Set(flat(fields).map((f) => f.id));
  while (segs.length > 0) {
    const id = segs.join('.');
    if (ids.has(id)) return id;
    segs.pop();
  }
  return undefined;
}
