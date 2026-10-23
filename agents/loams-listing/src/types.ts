// The listing's JSON form (schemas/marketplace/listing.v1.json), typed for
// the fields the tooling reads. The schema stays the authority on validity.

export type Kind = 'agent' | 'harness' | 'mcp-server';
export type Risk = 'read' | 'write' | 'egress' | 'destructive';

export interface Tool {
  name: string;
  description: string;
  risk: Risk;
  mcp_server?: string;
}

export interface ConfigInput {
  name: string;
  type: 'string' | 'number' | 'boolean' | 'enum';
  description?: string;
  required?: boolean;
  secret?: boolean;
  default?: unknown;
  values?: string[];
  env?: string;
}

export interface ArchiveFile {
  platform: string;
  url: string;
  sha256: string;
  path?: string;
}

export interface Artifact {
  type: 'npm' | 'pypi' | 'oci' | 'archive';
  name: string;
  version: string;
  integrity?: string;
  files?: ArchiveFile[];
}

export interface DataScope {
  kind: 'database' | 'collection' | 'stream' | 'bucket';
  name: string;
  access: 'read' | 'write';
}

export interface PlaceTemplate {
  data?: DataScope[];
  egress?: string[];
  always_ask?: string[];
  roles?: string[];
}

export interface Listing {
  $schema?: string;
  id: string;
  kind: Kind;
  version: string;
  title: string;
  summary: string;
  description?: string;
  icon?: string;
  licence: string;
  source: { repository: string; commit: string; path?: string };
  publisher: { name: string; url?: string };
  support?: string;
  security_contact?: string;
  framework?: { name: string; version?: string };
  language?: string;
  artifact?: Artifact;
  provenance?: { type: string; ref: string };
  runtime?: { kind: 'node' | 'python' | 'oci' | 'binary'; version?: string };
  entrypoint?: string;
  targets?: string[];
  surfaces?: Record<string, unknown>;
  loams?: Record<string, boolean>;
  connectors?: string[];
  mcp_servers?: { id: string; version: string }[];
  tools?: Tool[];
  place?: PlaceTemplate;
  models?: Record<string, unknown>;
  cost?: Record<string, number>;
  config?: ConfigInput[];
  conformance?: { level: string; report: string; report_sha256?: string };
  harness?: Record<string, unknown>;
  mcp_server?: {
    server: Record<string, unknown>;
    mode: 'local' | 'remote';
    args?: string[];
    remote_url?: string;
    transport?: string;
  };
  pricing: 'free';
}

export const LISTING_SCHEMA_ID = 'https://loams.dev/schemas/listing/v1.json';
