/**
 * GlitchTip adapter — public surface.
 *
 * The manifest is exported here rather than registered anywhere. Catalog
 * registration belongs to the integration step that composes the plugin host.
 *
 * PROVENANCE: the research for this adapter ran against the Angular 22.1 SPA,
 * whose `src/app/api/api-schema.d.ts` is generated from the server's own
 * `/api/openapi.json` — so paths and shapes are server truth. Against a live
 * instance, `GET /api/openapi.json` is the version-drift check.
 */

import type { PluginAgentSkill, PluginManifest } from "@loams-core/host";
import { GLITCHTIP_API_PREFIX, GLITCHTIP_ENV_PREFIX } from "./types.js";

export * from "./types.js";
export * from "./service.js";

export const GLITCHTIP_SKILLS: PluginAgentSkill[] = [
  {
    id: "listGlitchtipOrganizations",
    name: "List organizations",
    description:
      "List GlitchTip organizations and their `require2fa` flag. A `require2fa: false` org is worth " +
      "surfacing: an admin account there can be taken over with a single credential.",
    tags: ["glitchtip", "diagnostics", "read"],
    examples: ["listGlitchtipOrganizations {}"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listGlitchtipIssues",
    name: "List issues",
    description:
      "List grouped issues from /api/0/organizations/{org}/issues/. Beware: `id` is a STRING, `count` " +
      "is a STRINGIFIED number, `project` takes NUMERIC ids, and there is NO server-side `status` " +
      "filter — status filtering is client-side in the SPA.",
    tags: ["glitchtip", "issues", "read"],
    examples: [
      'listGlitchtipIssues {"orgSlug":"acme","sort":"-count","environment":["production"]}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getGlitchtipIssue",
    name: "Get issue",
    description:
      'Fetch one issue. `stats` is keyed by period ("24h", "14d") and each tuple is ' +
      "[epochSECONDS, count] — not milliseconds, not ISO.",
    tags: ["glitchtip", "issues", "read"],
    examples: ['getGlitchtipIssue {"issueId":"1234"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getGlitchtipIssueStats",
    name: "Get issue stats",
    description:
      "Time series for specific issues from /api/0/organizations/{org}/issues-stats/. `groups` is " +
      "REQUIRED — a non-empty list of numeric issue ids.",
    tags: ["glitchtip", "issues", "read"],
    examples: ['getGlitchtipIssueStats {"orgSlug":"acme","groups":[1234],"statsPeriod":"14d"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listGlitchtipIssueEvents",
    name: "List issue events",
    description:
      "List an issue's events. `tags` is an ARRAY OF SINGLE-KEY OBJECTS ([{k: v}]), not a map; " +
      "`projectID` is a NUMBER; `user` is unschema'd upstream and typed `unknown`.",
    tags: ["glitchtip", "issues", "read"],
    examples: ['listGlitchtipIssueEvents {"issueId":"1234"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listGlitchtipTransactionGroups",
    name: "List transaction groups",
    description:
      "Performance data from /api/0/organizations/{org}/transaction-groups/. `avgDuration`, `p50` and " +
      "`p95` are MILLISECONDS. `errorRate` and `throughput` are serializer-computed and read-only.",
    tags: ["glitchtip", "performance", "read"],
    examples: ['listGlitchtipTransactionGroups {"orgSlug":"acme"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listGlitchtipReleases",
    name: "List releases",
    description: "Releases and deploys from /api/0/organizations/{org}/releases/.",
    tags: ["glitchtip", "releases", "read"],
    examples: ['listGlitchtipReleases {"orgSlug":"acme"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listGlitchtipMonitors",
    name: "List monitors",
    description: "Uptime monitors and their checks.",
    tags: ["glitchtip", "monitors", "read"],
    examples: ['listGlitchtipMonitors {"orgSlug":"acme"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "checkGlitchtipHealth",
    name: "Check health",
    description:
      "Call GET /api/0/ for the version and the authenticated token's scopes. Doubles as the token " +
      "check: `auth` is populated only for a valid APIToken.",
    tags: ["glitchtip", "diagnostics", "read"],
    examples: ["checkGlitchtipHealth {}"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const glitchtipManifest: PluginManifest = {
  id: "glitchtip",
  name: "GlitchTip",
  description:
    "Error and performance monitoring: grouped issues, issue events, transaction groups, releases " +
    `and monitors. Reads the management API at ${GLITCHTIP_API_PREFIX} with an APIToken sent as ` +
    "`Authorization: Bearer <token>`. Tokens are scoped, but the scope strings are never enumerated " +
    "upstream, so an under-scoped token surfaces as a 403 rather than a pre-flight error.",
  version: "0.1.0",
  category: "observability",
  uiPath: "/plugins/glitchtip",
  order: 32,
  defaultEnabled: false,
  upstream: {
    product: "GlitchTip",
    envPrefix: GLITCHTIP_ENV_PREFIX,
  },
  agent: {
    name: "GlitchTip Agent",
    description:
      "Reads error and performance data from a GlitchTip deployment. Pagination is cursor-based and " +
      "carried entirely in response headers (`X-Hits`, `X-Max-Hits`, `Link`); list bodies are bare " +
      "JSON arrays.",
    version: "0.1.0",
    skills: GLITCHTIP_SKILLS,
  },
};
