// The factory host's Mastra instance (design §72 §6.2, D1428; plan FT1, FT0 Task 1).
//
// Storage is Mastra's own PostgresStore on Loams Postgres (`LOAMS_PG_URL`), the
// zero-package path of D1429. Workflows run on `@loams/mastra-resonate` against Loams
// Durable (`LOAMS_DURABLE_URL`) from Task 7; until that package is on npm (ruling
// FT0-0-1) the instance takes its workflows from the caller and registers none itself.
import './no-telemetry.js';
import type { MastraModelConfig } from '@mastra/core/llm';
import { Mastra } from '@mastra/core/mastra';
import type { MastraCompositeStore } from '@mastra/core/storage';
import { PostgresStore } from '@mastra/pg';

import { createLoamsBot } from './agents/loams-bot.js';

/** The environment the factory host reads, and nothing else (Task 3 builds it). */
export interface FactoryEnv {
  /** Loams Postgres, for Mastra storage and memory. */
  LOAMS_PG_URL?: string;
  /** Loams Durable (the Resonate protocol), for `factory.run` (Task 7). */
  LOAMS_DURABLE_URL?: string;
  /** The default model, as a Mastra model router string (`provider/model`). */
  LOAMS_FACTORY_MODEL?: string;
}

export interface FactoryOptions {
  model: MastraModelConfig;
  /** A storage to use instead of PostgresStore (tests). */
  storage?: MastraCompositeStore;
  /** Loams Postgres; required when no `storage` is given. */
  pgUrl?: string;
  /** Workflows to register (Task 7 passes `factory.run`, built on the Resonate engine). */
  workflows?: NonNullable<ConstructorParameters<typeof Mastra>[0]>['workflows'];
}

export class FactoryConfigError extends Error {
  override name = 'FactoryConfigError';
}

/** Read the host's settings from its environment, refusing an incomplete one. */
export function readFactoryEnv(env: FactoryEnv = process.env as FactoryEnv): {
  pgUrl: string;
  durableUrl: string | undefined;
  model: string;
} {
  const pgUrl = env.LOAMS_PG_URL?.trim();
  if (!pgUrl)
    throw new FactoryConfigError('LOAMS_PG_URL is not set: the factory host needs Loams Postgres');
  const model = env.LOAMS_FACTORY_MODEL?.trim();
  if (!model) {
    throw new FactoryConfigError(
      'LOAMS_FACTORY_MODEL is not set: choose a model as provider/model (no model is bundled)',
    );
  }
  return { pgUrl, durableUrl: env.LOAMS_DURABLE_URL?.trim() || undefined, model };
}

export function createFactoryMastra(options: FactoryOptions): Mastra {
  const storage =
    options.storage ??
    (() => {
      if (!options.pgUrl)
        throw new FactoryConfigError('createFactoryMastra needs pgUrl or storage');
      return new PostgresStore({ id: 'loams-factory', connectionString: options.pgUrl });
    })();
  const loamsBot = createLoamsBot({ model: options.model });
  return new Mastra({
    agents: { loamsBot },
    storage,
    ...(options.workflows ? { workflows: options.workflows } : {}),
    // The factory reports through its own exporters (MA2's LoamsExporter, Langfuse),
    // never Mastra's default telemetry.
    logger: false,
  });
}
