// Labels for the listing schema's strings (design §73 §2.4, §10.1). The page shows
// the schema value where a label is unknown, so a newer index still reads.

import type { ListingTier } from '@loams/agentd-client';
import {
  ConformanceLevelSchema,
  FrameworkNameSchema,
  KindSchema,
  LanguageSchema,
} from '@loams/proto/marketplace';
import type { BadgeVariant } from '@loams/ui';
import { enumStrings } from './wire.js';

export const KINDS = enumStrings(KindSchema);
export const FRAMEWORKS = enumStrings(FrameworkNameSchema);
export const LANGUAGES = enumStrings(LanguageSchema);
export const LEVELS = enumStrings(ConformanceLevelSchema);
export const TIERS: ListingTier[] = ['firstParty', 'verified', 'community'];

const KIND: Record<string, string> = {
  agent: 'Agent',
  harness: 'Harness',
  'mcp-server': 'MCP server',
};
export const kindLabel = (k: string) => KIND[k] ?? k;

const TIER: Record<ListingTier, string> = {
  firstParty: 'First-party',
  verified: 'Verified',
  community: 'Community',
};
export const tierLabel = (t: ListingTier) => TIER[t] ?? t;
export const tierVariant = (t: ListingTier): BadgeVariant =>
  t === 'firstParty' ? 'success' : t === 'verified' ? 'info' : 'neutral';

/** The integration levels of §2.4, by the lowest conformance level passed. */
const LEVEL: Record<string, string> = {
  core: 'Runs on Loams',
  compatible: 'Loams-compatible',
  native: 'Loams-native',
};
export const levelLabel = (l: string) => LEVEL[l] ?? l;

const FRAMEWORK: Record<string, string> = {
  mastra: 'Mastra',
  'pydantic-ai': 'Pydantic AI',
  langgraph: 'LangGraph',
  crewai: 'CrewAI',
  'openai-agents': 'OpenAI Agents',
  'google-adk': 'Google ADK',
  'microsoft-agent-framework': 'Microsoft Agent Framework',
  eve: 'eve',
  other: 'Other framework',
};
export const frameworkLabel = (f: string) => FRAMEWORK[f] ?? f;

const LANGUAGE: Record<string, string> = {
  typescript: 'TypeScript',
  javascript: 'JavaScript',
  python: 'Python',
  go: 'Go',
  rust: 'Rust',
  java: 'Java',
  kotlin: 'Kotlin',
  csharp: 'C#',
  zig: 'Zig',
  other: 'Other language',
};
export const languageLabel = (l: string) => LANGUAGE[l] ?? l;

/** Risk, most dangerous first (the plan's tool order). */
const RISK: Record<string, { label: string; variant: BadgeVariant }> = {
  destructive: { label: 'Destructive', variant: 'danger' },
  egress: { label: 'Reaches the network', variant: 'warning' },
  write: { label: 'Writes', variant: 'warning' },
  read: { label: 'Reads', variant: 'neutral' },
};
export const riskLabel = (r: string) => RISK[r]?.label ?? (r || 'Unknown risk');
export const riskVariant = (r: string): BadgeVariant => RISK[r]?.variant ?? 'danger';
