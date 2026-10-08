/**
 * OpenPanel adapter — public surface.
 *
 * The manifest is exported here rather than registered anywhere. Catalog
 * registration belongs to the integration step that composes the plugin host.
 */

import type { PluginAgentSkill, PluginManifest } from "@loams-core/host";
import { OPENPANEL_ENV_PREFIX, OPENPANEL_REQUIRED_CLIENT_TYPE } from "./types.js";

export * from "./types.js";
export * from "./service.js";

export const OPENPANEL_SKILLS: PluginAgentSkill[] = [
  {
    id: "openPanelOverview",
    name: "Traffic overview",
    description:
      "Bounce rate, unique visitors, sessions and revenue over a window from /insights/:id/overview. " +
      "`avg_session_duration` is SECONDS and `bounce_rate`/`views_per_session` are rounded to 2dp.",
    tags: ["openpanel", "insights", "read"],
    examples: ['openPanelOverview {"projectId":"...","range":"7d","interval":"day"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "openPanelFunnel",
    name: "Funnel",
    description:
      "Conversion funnel from /insights/:id/funnel. Needs 2..10 ordered event names; windowHours is " +
      "1..720 (default 24). An empty funnel is a zero shape, not an error.",
    tags: ["openpanel", "insights", "read"],
    examples: [
      'openPanelFunnel {"projectId":"...","steps":["view_item","add_to_cart","checkout"]}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "openPanelRetention",
    name: "Retention",
    description:
      "Cohort retention from /insights/:id/retention. `retention` is a PERCENTAGE 0..100 float, not a " +
      "0..1 fraction.",
    tags: ["openpanel", "insights", "read"],
    examples: ['openPanelRetention {"projectId":"..."}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "openPanelActiveUsers",
    name: "Active users",
    description: "DAU/WAU/MAU series from /insights/:id/active_users. `days` is 1..90, default 7.",
    tags: ["openpanel", "insights", "read"],
    examples: ['openPanelActiveUsers {"projectId":"...","days":7}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "openPanelPagesPerformance",
    name: "Page performance",
    description:
      "Per-page sessions, pageviews and SEO signals from /insights/:id/pages/performance. " +
      "`avg_duration` here is MINUTES — unlike overview's seconds-valued `avg_session_duration`.",
    tags: ["openpanel", "insights", "read"],
    examples: ['openPanelPagesPerformance {"projectId":"..."}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "openPanelTopPages",
    name: "Top pages",
    description:
      "Most-visited pages from /insights/:id/pages/top. Not paginated — `limit` only, max 100.",
    tags: ["openpanel", "insights", "read"],
    examples: ['openPanelTopPages {"projectId":"..."}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "openPanelListEvents",
    name: "List exported events",
    description:
      "Events from /export/events: transformed camelCase with ISO dates. The ONLY paginated read " +
      "endpoint — `page` (1-based) plus `limit` (1..1000). There is no cursor in OpenPanel.",
    tags: ["openpanel", "export", "read"],
    examples: ['openPanelListEvents {"projectId":"...","page":1,"limit":50}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "openPanelLiveVisitors",
    name: "Live visitors",
    description: "Current visitor count from /insights/:id/live → `{visitors: number}`.",
    tags: ["openpanel", "insights", "read"],
    examples: ['openPanelLiveVisitors {"projectId":"..."}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "checkOpenPanelHealth",
    name: "Check health",
    description:
      "Call GET /healthcheck for the deployment status. Remember the public URL needs the /api prefix " +
      "and a direct container address must not have it.",
    tags: ["openpanel", "diagnostics", "read"],
    examples: ["checkOpenPanelHealth {}"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const openPanelManifest: PluginManifest = {
  id: "openpanel",
  name: "OpenPanel",
  description:
    "Product analytics: traffic, funnels, retention, page performance and exported events. " +
    "Authenticates with the two custom headers `openpanel-client-id` and `openpanel-client-secret`; " +
    "the client id must be a UUIDv4-shaped lowercase string and the client type must NOT be `write` " +
    "for /export/* and /insights/*. /manage/* additionally requires `root`.",
  version: "0.1.0",
  category: "product-analytics",
  uiPath: "/plugins/openpanel",
  order: 31,
  defaultEnabled: false,
  upstream: {
    product: "OpenPanel",
    envPrefix: OPENPANEL_ENV_PREFIX,
  },
  agent: {
    name: "OpenPanel Agent",
    description:
      "Reads product analytics from an OpenPanel deployment. Note the two naming conventions: /export/* " +
      `is camelCase with ISO dates, /insights/* is snake_case with unzoned ClickHouse dates. Requires ` +
      `a client of type \`${OPENPANEL_REQUIRED_CLIENT_TYPE.insights}\` or higher.`,
    version: "0.1.0",
    skills: OPENPANEL_SKILLS,
  },
};
