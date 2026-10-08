import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { ConnectorDetail, ConnectorSummary, LoamsDesktopApi } from '@loams/desktop/contracts';
import { validateConfig } from '@loams/desktop/validate';
import { parse } from 'yaml';

const root = resolve(import.meta.dirname, '../../../..');

/** A connector read straight from connectors/registry and connectors/schemas. */
export function load(id: string): { summary: ConnectorSummary; detail: ConnectorDetail } {
  const manifest = parse(readFileSync(`${root}/connectors/registry/${id}.yaml`, 'utf8'));
  const schema = JSON.parse(readFileSync(`${root}/connectors/schemas/${id}.config.json`, 'utf8'));
  const stub = !schema.properties;
  return {
    summary: {
      id,
      name: manifest.name,
      category: manifest.category,
      status: manifest.status,
      runtime: { kind: manifest.runtime.kind, ref: manifest.runtime.ref },
      source: manifest.capabilities?.source ? ['streaming'] : null,
      sink: manifest.capabilities?.sink ? ['streaming'] : null,
      modes: ['streaming'],
      auth: manifest.auth ?? [],
      licence: manifest.licence.component,
      stub: stub || 'x-loams-generated-by' in schema,
    },
    detail: { manifest, schema },
  };
}

export function fakeDesktop(ids: string[]) {
  const all = ids.map(load);
  const clipboard: string[] = [];
  const api = {
    connectors: {
      catalog: async () => all.map((a) => a.summary),
      get: async (id: string) => {
        const f = all.find((a) => a.summary.id === id);
        return f
          ? { ok: true as const, value: f.detail }
          : { ok: false as const, code: 'not_found', message: `No connector "${id}".` };
      },
      validate: async (id: string, config: unknown) => {
        const f = all.find((a) => a.summary.id === id);
        return f
          ? { ok: true as const, value: validateConfig(f.detail.schema, config) }
          : { ok: false as const, code: 'not_found', message: 'nope' };
      },
    },
    shell: {
      clipboardWrite: async (t: string) => void clipboard.push(t),
      openExternal: async () => ({ ok: true as const, value: undefined }),
    },
  } as unknown as LoamsDesktopApi;
  return { api, clipboard, all };
}
