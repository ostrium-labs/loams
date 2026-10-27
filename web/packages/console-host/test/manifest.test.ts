// The manifest's `modes` field (SD1 Task 3, §53 §4.1, D901), the agentd
// permissions and the agentd injection rule (SD1 Task 15), and the JSON
// Schema kept in step with the validator.

import { describe, expect, it } from 'vite-plus/test';
import schema from '../plugin-manifest.schema.json';
import {
  boot,
  decideCall,
  injectsAgentd,
  MANIFEST_KEYS,
  ManifestError,
  PERMISSIONS,
  validateManifest,
} from '../src/index.js';
import { createMockControl } from '../src/testing/index.js';

function pkg(plugin: Record<string, unknown>, name = '@loams/plugin-jobs') {
  return {
    name,
    version: '0.1.0',
    loams: { plugin: { kind: 'console', entry: '.', tier: 'first-party', ...plugin } },
  };
}

describe('manifest modes', () => {
  it('manifest_modes_default_console', () => {
    expect(validateManifest(pkg({})).modes).toEqual(['console']);
    expect(validateManifest(pkg({ modes: ['studio'] })).modes).toEqual(['studio']);
    expect(validateManifest(pkg({ modes: ['console', 'studio'] })).modes).toEqual([
      'console',
      'studio',
    ]);
  });

  it('manifest_modes_unknown_refused', () => {
    expect(() => validateManifest(pkg({ modes: ['admin'] }))).toThrow(ManifestError);
    expect(() => validateManifest(pkg({ modes: ['admin'] }))).toThrow(/unknown mode "admin"/);
    expect(() => validateManifest(pkg({ modes: [] }))).toThrow(/at least one mode/);
    expect(() => validateManifest(pkg({ modes: 'studio' }))).toThrow(/list of strings/);
    expect(() => validateManifest(pkg({ modes: ['studio', 'studio'] }))).toThrow(/twice/);
  });

  it('schema_and_validator_agree', () => {
    const props = schema.properties as Record<string, unknown>;
    // Every key the validator accepts is in the schema, and the other way round.
    expect([...MANIFEST_KEYS].sort()).toEqual(Object.keys(props).sort());
    expect(() => validateManifest(pkg({ notInSchema: 1 }))).toThrow(/unknown manifest key/);
    // modes: the same enum, unique, at least one.
    expect(props.modes).toEqual({
      type: 'array',
      items: { enum: ['console', 'studio'] },
      uniqueItems: true,
      minItems: 1,
      description: expect.any(String),
    });
    // permissions: the schema's enum is the validator's list.
    const permissions = props.permissions as { items: { enum: string[] } };
    expect(permissions.items.enum).toEqual([...PERMISSIONS]);
  });
});

describe('agentd permissions and injection', () => {
  it('knows the agentd permissions', () => {
    expect(
      validateManifest(pkg({ permissions: ['agentd:use', 'agentd:git-write'] })).permissions,
    ).toEqual(['agentd:use', 'agentd:git-write']);
  });

  it('injectsAgentd spots rpc.agentd.* and the desktop bridge', () => {
    expect(injectsAgentd(['router', 'rpc.agentd.projects'])).toEqual(['rpc.agentd.projects']);
    expect(injectsAgentd(['desktop'])).toEqual(['desktop']);
    expect(injectsAgentd(['router', 'rpc.approvals'])).toEqual([]);
  });

  it('third_party_cannot_inject_agentd', async () => {
    const control = createMockControl();
    const handle = await boot({
      catalog: [
        { id: 'evil', name: '@acme/plugin-evil' },
        { id: 'studio', name: '@loams/plugin-studio' },
      ],
      manifests: [
        pkg({ tier: 'third-party', inject: ['rpc.agentd.projects'] }, '@acme/plugin-evil'),
        pkg({ inject: [], modes: ['studio'] }, '@loams/plugin-studio'),
      ],
      modules: { '@loams/plugin-studio': async () => ({ apply() {} }) },
      platform: {
        name: 'platform-test',
        apply(ctx) {
          ctx.provide('platform', {
            kind: 'web',
            fetch: globalThis.fetch,
            baseUrl: 'mock:',
            openExternal: async () => {},
            notify: async () => {},
            clipboardWrite: async () => {},
          });
          ctx.provide('transport', control.transport);
        },
      },
      sandboxScripts: { '@acme/plugin-evil': '/plugins/evil.js' },
      grant: (m) => m.permissions,
      storage: { getItem: () => null, setItem: () => {} },
      initialPath: () => '/',
      navigate: () => {},
    });
    const evil = handle.plugins().find((p) => p.id === 'evil');
    expect(evil?.status).toBe('failed');
    expect(evil?.reason).toMatch(/third-party plugins never get rpc\.agentd\.projects/);
    // And the sandbox bridge refuses any agentd call outright.
    const decision = decideCall(
      { services: ['rpc.agentd.projects'], permissions: ['agentd:use'] },
      'rpc.agentd.projects',
      'listRepos',
    );
    expect(decision.ok).toBe(false);
    await handle.dispose();
  });
});
