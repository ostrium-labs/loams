/**
 * Langfuse adapter — public surface.
 *
 * The manifest is exported here rather than registered anywhere. Catalog
 * registration belongs to the integration step that composes the plugin host;
 * an adapter that registered itself would make load order a function of import
 * order.
 */

import type { PluginAgentSkill, PluginManifest } from "@loams-core/host";
import { LANGFUSE_API_PREFIX, LANGFUSE_ENV_PREFIX, LANGFUSE_LEGACY_REMOVED_ON } from "./types.js";

export * from "./types.js";
export * from "./service.js";

export const LANGFUSE_SKILLS: PluginAgentSkill[] = [
  {
    id: "runLangfuseMetrics",
    name: "Run a metrics query",
    description:
      "Aggregate observations or scores with /api/public/v2/metrics. Returns the derived column keys " +
      "alongside the rows, because result columns are named {aggregation}_{measure} rather than after " +
      "the measure.",
    tags: ["langfuse", "metrics", "read"],
    examples: [
      'runLangfuseMetrics {"view":"observations","metrics":["latency"],"fromTimestamp":"2026-01-01T00:00:00Z","toTimestamp":"2026-01-08T00:00:00Z","timeDimension":{"granularity":"day"}}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listLangfuseObservations",
    name: "List observations",
    description:
      "Read observations from /api/public/v2/observations, cursor-paginated. Timestamps are ISO-8601 " +
      "strings; latency and totalCost are only present when the matching `fields` group is requested.",
    tags: ["langfuse", "observations", "read"],
    examples: [
      'listLangfuseObservations {"fields":["core","basic","metrics","usage"],"environment":["production"],"limit":100}',
    ],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listLangfuseScores",
    name: "List scores",
    description:
      "Read scores from /api/public/v3/scores, cursor-paginated. `value` is polymorphic: number for " +
      "NUMERIC, boolean for BOOLEAN, string for CATEGORICAL/TEXT/CORRECTION.",
    tags: ["langfuse", "scores", "read"],
    examples: ['listLangfuseScores {"limit":100,"fields":["core","details"]}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listLangfuseDatasets",
    name: "List datasets",
    description: "List evaluation datasets from /api/public/v2/datasets.",
    tags: ["langfuse", "datasets", "read"],
    examples: ["listLangfuseDatasets {}"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listLangfuseDatasetItems",
    name: "List dataset items",
    description: "List items of a dataset from /api/public/v2/dataset-items.",
    tags: ["langfuse", "datasets", "read"],
    examples: ['listLangfuseDatasetItems {"datasetName":"qa-regression"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listLangfuseExperiments",
    name: "List experiments",
    description:
      "List experiments and their items from /api/public/v2/experiments and /experiment-items.",
    tags: ["langfuse", "experiments", "read"],
    examples: ["listLangfuseExperiments {}"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "listLangfuseScoreConfigs",
    name: "List score configs",
    description: "List score configurations from /api/public/v2/score-configs.",
    tags: ["langfuse", "scores", "read"],
    examples: ["listLangfuseScoreConfigs {}"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "checkLangfuseHealth",
    name: "Check health",
    description:
      "Call GET /api/public/health (unauthenticated) for the Langfuse version and status. The version " +
      "is how you confirm the deployment still serves /v2/metrics and /v3/scores.",
    tags: ["langfuse", "diagnostics", "read"],
    examples: ["checkLangfuseHealth {}"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const langfuseManifest: PluginManifest = {
  id: "langfuse",
  name: "Langfuse",
  description:
    "LLM observability: latency, cost, usage and scores. Metrics come from /api/public/v2/metrics; " +
    `observations from /v2/observations and scores from /v3/scores. The legacy v3 surface is removed ` +
    `on ${LANGFUSE_LEGACY_REMOVED_ON} and this adapter refuses to call it.`,
  version: "0.1.0",
  category: "observability",
  uiPath: "/plugins/langfuse",
  order: 30,
  defaultEnabled: false,
  upstream: {
    product: "Langfuse",
    envPrefix: LANGFUSE_ENV_PREFIX,
  },
  agent: {
    name: "Langfuse Agent",
    description: `Reads LLM traces and scores from a Langfuse deployment's ${LANGFUSE_API_PREFIX} API.`,
    version: "0.1.0",
    skills: LANGFUSE_SKILLS,
  },
};
