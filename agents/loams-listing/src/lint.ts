// `loams-listing lint`: the schema, then the rules a JSON Schema cannot say
// (design 73 sections 3.3, 3.6, 7.4 and 10.2).
import { validateAgainst } from './schema.js';
import { BUN_PINNED, satisfies } from './semver.js';
import type { Listing, Tool } from './types.js';

export type Severity = 'error' | 'warning';

export interface LintIssue {
  rule: string;
  severity: Severity;
  /** JSON Pointer to the offending value ('' for the listing). */
  path: string;
  message: string;
}

export interface LintOptions {
  /** MK0's index policy: only the reserved first-party namespaces (plan MK1 Global Constraints). */
  firstPartyOnly?: boolean;
}

/** The namespaces reserved for Ostrium Labs (design 73 section 6). */
export const FIRST_PARTY_NAMESPACES = ['ostrium', 'loams'] as const;

/** Every rule `lint` can report, with what it checks. */
export const RULES: Record<string, string> = {
  schema: 'The listing matches schemas/marketplace/listing.v1.json.',
  'namespace-first-party': 'Only the ostrium and loams namespaces are accepted (MK0).',
  'tool-names-unique': 'Each tool is declared once.',
  'always-ask-declared': 'place.always_ask names declared tools only.',
  'tool-mcp-server-declared': "A tool's mcp_server is one of the listing's mcp_servers.",
  'mcp-server-self': "An MCP server's tools come from itself, so they name no mcp_server.",
  'remote-url-in-server-json': "A remote MCP server's URL is one of its server.json remotes.",
  'server-json-version': "The embedded server.json's version is the listing's version.",
  'npm-version-matches': "An npm artifact's version is the listing's version.",
  'config-names-unique': 'Configuration names and environment variables are unique.',
  'config-env-reserved': 'Configuration never sets a variable of the environment contract (D1453).',
  'mastra-profile': 'A Mastra agent runs on node and is written in TypeScript or JavaScript.',
  'engine-version': `A Bun listing's runtime.version admits Bun ${BUN_PINNED}, the version agentd runs (D1791, D1808).`,
  'egress-declared': 'A listing with egress tools declares the hosts it reaches.',
  'tool-poisoning': 'Tool descriptions hold no instructions to the model and no hidden text.',
  'archive-platforms-unique': 'An archive has one file per platform.',
  'walrus-sha256-matches': "A Walrus mirror of an archive file carries that file's sha256 (D1812).",
  'data-scopes-unique': 'Each data scope is declared once.',
};

/** Variables agentd sets for every agent (design 73 section 3.3); a config input may not shadow them. */
const CONTRACT_ENV = /^(LOAMS_.*|HOME|TMPDIR|HTTPS?_PROXY|NO_PROXY|OPENAI_BASE_URL|ANTHROPIC_BASE_URL|OPENAI_API_KEY|ANTHROPIC_API_KEY|PATH|NODE_OPTIONS|PYTHONPATH)$/;

/** Text in a tool description that speaks to the model rather than describing the tool. */
const POISON: RegExp[] = [
  /ignore (all |any )?(previous|prior|above) (instructions|prompts?)/i,
  /\b(system prompt|developer message)\b/i,
  /<\s*(important|system|instructions?)\s*>/i,
  /do not (tell|inform|mention (this )?to) the user/i,
  /<!--/,
  // Zero-width and bidirectional control characters hide text from people.
  /[​-‏‪-‮⁠-⁤﻿]/,
];

function duplicates(values: string[]): string[] {
  const seen = new Set<string>();
  const dup = new Set<string>();
  for (const value of values) {
    if (seen.has(value)) dup.add(value);
    seen.add(value);
  }
  return [...dup];
}

export function lint(doc: unknown, options: LintOptions = {}): LintIssue[] {
  const issues: LintIssue[] = [];
  const error = (rule: string, path: string, message: string) =>
    issues.push({ rule, severity: 'error', path, message });
  const warning = (rule: string, path: string, message: string) =>
    issues.push({ rule, severity: 'warning', path, message });

  const schemaIssues = validateAgainst('listing.v1.json', doc);
  for (const issue of schemaIssues) error('schema', issue.path, issue.message);
  // The semantic rules read fields the schema guarantees; skip them on a broken listing.
  if (schemaIssues.length > 0) return issues;
  const listing = doc as Listing;

  const namespace = listing.id.split('/')[0] ?? '';
  if (options.firstPartyOnly && !(FIRST_PARTY_NAMESPACES as readonly string[]).includes(namespace)) {
    error('namespace-first-party', '/id', `namespace ${namespace} is not first-party; third-party listings open in MK1`);
  }

  const tools: Tool[] = listing.tools ?? [];
  for (const name of duplicates(tools.map((t) => t.name))) {
    error('tool-names-unique', '/tools', `tool ${name} is declared more than once`);
  }
  const toolNames = new Set(tools.map((t) => t.name));
  (listing.place?.always_ask ?? []).forEach((name, i) => {
    if (!toolNames.has(name)) error('always-ask-declared', `/place/always_ask/${i}`, `${name} is not a declared tool`);
  });

  const dependencyIds = new Set((listing.mcp_servers ?? []).map((d) => d.id));
  tools.forEach((tool, i) => {
    if (tool.mcp_server === undefined) return;
    if (listing.kind === 'mcp-server') {
      error('mcp-server-self', `/tools/${i}/mcp_server`, 'an MCP server listing declares its own tools');
    } else if (!dependencyIds.has(tool.mcp_server)) {
      error('tool-mcp-server-declared', `/tools/${i}/mcp_server`, `${tool.mcp_server} is not in mcp_servers`);
    }
  });

  if (listing.mcp_server) {
    const server = listing.mcp_server.server as { version?: unknown; remotes?: { url?: unknown }[] };
    if (listing.mcp_server.mode === 'remote') {
      const urls = (server.remotes ?? []).map((r) => r.url);
      if (!urls.includes(listing.mcp_server.remote_url)) {
        error('remote-url-in-server-json', '/mcp_server/remote_url', 'remote_url is not one of server.json remotes[].url');
      }
    }
    if (server.version !== listing.version) {
      error('server-json-version', '/mcp_server/server/version', `server.json version ${String(server.version)} is not ${listing.version}`);
    }
  }

  if (listing.artifact?.type === 'npm' && listing.artifact.version !== listing.version) {
    error('npm-version-matches', '/artifact/version', `npm version ${listing.artifact.version} is not ${listing.version}`);
  }

  const config = listing.config ?? [];
  for (const name of duplicates(config.map((c) => c.name))) {
    error('config-names-unique', '/config', `config ${name} is declared more than once`);
  }
  for (const env of duplicates(config.flatMap((c) => (c.env ? [c.env] : [])))) {
    error('config-names-unique', '/config', `environment variable ${env} is set by more than one input`);
  }
  config.forEach((input, i) => {
    if (input.env && CONTRACT_ENV.test(input.env)) {
      error('config-env-reserved', `/config/${i}/env`, `${input.env} belongs to the environment contract`);
    }
  });

  if (listing.framework?.name === 'mastra') {
    if (listing.runtime?.kind !== 'node') error('mastra-profile', '/runtime/kind', 'a Mastra agent runs on the node runtime');
    if (listing.language !== 'typescript' && listing.language !== 'javascript') {
      error('mastra-profile', '/language', 'a Mastra agent is TypeScript or JavaScript');
    }
  }

  if (listing.runtime?.engine === 'bun') {
    const range = listing.runtime.version ?? `>=${BUN_PINNED}`;
    if (!satisfies(range, BUN_PINNED)) {
      error('engine-version', '/runtime/version', `bun ${BUN_PINNED} is not in ${range}; is it a Node range?`);
    }
  }

  if (tools.some((t) => t.risk === 'egress') && (listing.place?.egress ?? []).length === 0) {
    warning('egress-declared', '/place/egress', 'egress tools are declared but no egress hosts');
  }

  tools.forEach((tool, i) => {
    for (const [field, text] of [['name', tool.name], ['description', tool.description]] as const) {
      if (POISON.some((pattern) => pattern.test(text))) {
        error('tool-poisoning', `/tools/${i}/${field}`, `tool ${tool.name}'s ${field} looks like an instruction to the model or hides text`);
      }
    }
  });

  if (listing.artifact?.type === 'archive') {
    for (const platform of duplicates((listing.artifact.files ?? []).map((f) => f.platform))) {
      error('archive-platforms-unique', '/artifact/files', `${platform} has more than one file`);
    }
    (listing.artifact.files ?? []).forEach((file, i) => {
      (file.locations ?? []).forEach((location, j) => {
        if (location.kind === 'walrus' && location.sha256 !== file.sha256) {
          error('walrus-sha256-matches', `/artifact/files/${i}/locations/${j}/sha256`, `the Walrus mirror's sha256 is not the ${file.platform} file's`);
        }
      });
    });
  }

  for (const scope of duplicates((listing.place?.data ?? []).map((d) => `${d.kind}:${d.name}`))) {
    error('data-scopes-unique', '/place/data', `data scope ${scope} is declared more than once`);
  }

  return issues;
}

export function hasErrors(issues: LintIssue[]): boolean {
  return issues.some((issue) => issue.severity === 'error');
}

export function formatIssue(file: string, issue: LintIssue): string {
  return `${file}${issue.path ? `#${issue.path}` : ''}: ${issue.severity} [${issue.rule}] ${issue.message}`;
}
