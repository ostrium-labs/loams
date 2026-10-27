import { InMemoryStore } from '@mastra/core/storage';
import { createMockModel } from '@mastra/core/test-utils/llm-mock';
import { PostgresStore } from '@mastra/pg';
import { describe, expect, it } from 'vite-plus/test';

import {
  createFactoryMastra,
  createLoamsBot,
  FactoryConfigError,
  LOAMS_BOT_ID,
  readFactoryEnv,
} from '../src/index.js';

const mockModel = (text: string) => createMockModel({ mockText: text, version: 'v2' });

describe('the factory host', () => {
  it('mastra_instance_builds', () => {
    // PostgresStore connects lazily, so building the instance needs no database.
    const mastra = createFactoryMastra({
      model: mockModel('hi'),
      pgUrl: 'postgres://loams@127.0.0.1:1/loams',
    });
    const bot = mastra.getAgent('loamsBot');
    expect(bot.id).toBe(LOAMS_BOT_ID);
    expect(bot.name).toBe('Loams Bot');
    expect(mastra.getStorage()).toBeInstanceOf(PostgresStore);
  });

  it('refuses to build without storage', () => {
    expect(() => createFactoryMastra({ model: mockModel('hi') })).toThrow(FactoryConfigError);
  });

  it('loams_bot_answers_with_mock_model', async () => {
    const mastra = createFactoryMastra({
      model: mockModel('Here is a plan for rate limiting the signup endpoint.'),
      storage: new InMemoryStore(),
    });
    const result = await mastra.getAgent('loamsBot').generate('add rate limiting to signup');
    expect(result.text).toBe('Here is a plan for rate limiting the signup endpoint.');
  });

  it('streams the same answer', async () => {
    const bot = createLoamsBot({ model: mockModel('streamed') });
    const stream = await bot.stream('hello');
    let text = '';
    for await (const chunk of stream.textStream) text += chunk;
    expect(text).toBe('streamed');
  });
});

describe('telemetry', () => {
  it('mastra_telemetry_is_off_by_default', () => {
    expect(process.env.MASTRA_TELEMETRY_DISABLED).toBe('1');
  });
});

describe('readFactoryEnv', () => {
  it('reads the three settings', () => {
    expect(
      readFactoryEnv({
        LOAMS_PG_URL: ' postgres://x/y ',
        LOAMS_DURABLE_URL: 'http://127.0.0.1:8001',
        LOAMS_FACTORY_MODEL: 'anthropic/claude-sonnet-4-5',
      }),
    ).toEqual({
      pgUrl: 'postgres://x/y',
      durableUrl: 'http://127.0.0.1:8001',
      model: 'anthropic/claude-sonnet-4-5',
    });
  });

  it('needs Postgres and a model, not Durable', () => {
    expect(() => readFactoryEnv({ LOAMS_FACTORY_MODEL: 'a/b' })).toThrow(/LOAMS_PG_URL/);
    expect(() => readFactoryEnv({ LOAMS_PG_URL: 'postgres://x/y' })).toThrow(/LOAMS_FACTORY_MODEL/);
    expect(
      readFactoryEnv({ LOAMS_PG_URL: 'postgres://x/y', LOAMS_FACTORY_MODEL: 'a/b' }).durableUrl,
    ).toBeUndefined();
  });
});
