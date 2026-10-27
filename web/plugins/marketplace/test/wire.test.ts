// The Connect form maps to the ndjson form agentd's fixtures show (plan MK1 Task 4b,
// ruling 3), the way loams-agentd-connect's map/marketplace.rs maps it.

import { create } from '@bufbuild/protobuf';
import * as pb from '@loams/proto/agentd';
import * as listing from '@loams/proto/marketplace';
import { describe, expect, it } from 'vite-plus/test';
import { enumNumber, enumString, planOf, prefixOf, searchRequest, summaryOf } from '../src/wire.js';
import { PLAN, SEARCH } from './fake.js';

describe('enum names', () => {
  it('maps schema strings to proto enums and back', () => {
    expect(prefixOf('FrameworkName')).toBe('FRAMEWORK_NAME_');
    expect(enumString(listing.KindSchema, listing.Kind.MCP_SERVER)).toBe('mcp-server');
    expect(enumNumber(listing.KindSchema, 'mcp-server')).toBe(listing.Kind.MCP_SERVER);
    expect(enumString(listing.FrameworkNameSchema, listing.FrameworkName.PYDANTIC_AI)).toBe(
      'pydantic-ai',
    );
    expect(enumString(listing.RiskSchema, 'RISK_DESTRUCTIVE')).toBe('destructive');
    expect(enumString(listing.KindSchema, 0)).toBeUndefined();
    expect(enumString(listing.PlatformSchema, listing.Platform.LINUX_X86_64)).toBe('linux-x86_64');
    expect(enumNumber(listing.PlatformSchema, 'linux-x86_64')).toBe(listing.Platform.LINUX_X86_64);
    expect(enumNumber(listing.KindSchema, 'nonsense')).toBe(0);
  });

  it('builds the search request', () => {
    expect(
      searchRequest({
        query: 'git',
        kinds: ['harness'],
        frameworks: ['mastra'],
        languages: ['typescript'],
        minLevel: 'compatible',
        tiers: ['firstParty'],
        source: null,
      }),
    ).toEqual({
      query: 'git',
      kinds: [listing.Kind.HARNESS],
      frameworks: [listing.FrameworkName.MASTRA],
      languages: [listing.Language.TYPESCRIPT],
      minLevel: listing.ConformanceLevel.COMPATIBLE,
      tiers: [pb.ListingTier.FIRST_PARTY],
      source: '',
    });
  });
});

describe('the Connect plan', () => {
  it('is the PlanInstall fixture', () => {
    const p = PLAN;
    const message = create(pb.InstallPlanSchema, {
      digest: p.digest,
      version: p.version,
      summary: {
        id: p.summary.id,
        source: p.summary.source,
        kind: listing.Kind.MCP_SERVER,
        title: p.summary.title,
        summary: p.summary.summary,
        tier: pb.ListingTier.FIRST_PARTY,
        latest: '2.0.3',
        language: listing.Language.GO,
        level: listing.ConformanceLevel.CORE,
        revocation: {
          severity: pb.RevocationSeverity.WARN,
          reason: 'An upstream advisory affects some tools.',
          versions: '>=2.0.0, <2.1.0',
          advisory: 'https://github.com/advisories/GHSA-0000-0000-0000',
        },
      },
      place: { alwaysAsk: ['merge_pull_request'], egress: ['api.github.com'] },
      tools: [
        {
          name: 'merge_pull_request',
          description: 'Merges a pull request.',
          risk: listing.Risk.DESTRUCTIVE,
          alwaysAsks: true,
        },
        {
          name: 'get_issue',
          description: 'Reads an issue.',
          risk: listing.Risk.READ,
          mcpServer: 'io.github.github/github-mcp-server',
        },
      ],
      egress: ['api.github.com'],
      dependencies: [{ id: 'ostrium/plane-mcp-server', version: '>=0.3.0', installed: true }],
      secrets: [{ name: 'token', description: 'A GitHub personal access token.', required: true }],
      licence: 'MIT',
      provenance: {
        type: listing.ProvenanceType.GITHUB_ATTESTATION,
        ref: 'https://api.github.com/repos/github/github-mcp-server/attestations',
      },
      sourceRepository: 'https://github.com/github/github-mcp-server',
      versionAgeDays: 4,
      cannot: ['Reach any host but api.github.com.'],
    });
    expect(planOf(message)).toEqual(PLAN);
  });

  it('never shows an unspecified tier as first-party', () => {
    expect(summaryOf(create(pb.ListingSummarySchema, { id: 'x' })).tier).toBe('community');
    expect(SEARCH.results[0]?.tier).toBe('firstParty');
  });
});
