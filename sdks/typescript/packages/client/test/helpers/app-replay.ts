// Replays the SDKP app scenario (`sdks/fixtures/recorded/app/`) through the
// **public facade**, against the fixture server (`sdks/fixtures/app-scenario.md`
// says the same in prose).
//
// For each step: the facade call it names (`documents.write`, `vector.search`)
// is made on a `Loams` client, with the request the recording sent — built with
// the builders when the step names a builder case, so the replay proves the
// builder writes the recorded request. The fixture server compares the body it
// receives with the recording, so a facade that sent anything else fails here.
// The answer is compared with the recorded one, or, for a refusal, the typed
// error's code and reason with the recorded ones.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fromJson, toJson, type JsonObject, type JsonValue } from '@bufbuild/protobuf';
import { MODULES, SERVICE_DESCRIPTORS } from '../../src/gen/facade.js';
import { Loams } from '../../src/loams.js';
import { LoamsError } from '../../src/runtime/errors.js';
import { args, builderCase, filter, schema, search, type Call } from './builders.js';
import { FIXTURES } from './server.js';

/** The app fixtures the manifest names, in order. */
export function appFixtureNames(): string[] {
  const manifest = JSON.parse(readFileSync(join(FIXTURES, 'manifest.json'), 'utf8')) as {
    fixtures: { name: string }[];
  };
  return manifest.fixtures.map((f) => f.name).filter((name) => name.startsWith('app_'));
}

interface AppStep {
  about?: string;
  call: string;
  builder?: string;
  request: { body?: string };
  response: { status: number; body?: JsonValue };
  expect: { status: number; reason?: string };
}

interface AppFixture {
  name: string;
  steps: AppStep[];
}

function readAppFixture(name: string): AppFixture {
  return JSON.parse(readFileSync(join(FIXTURES, 'recorded', 'app', `${name}.json`), 'utf8')) as AppFixture;
}

/** The request JSON a step sends, built with the builders where it names one. */
function requestJson(step: AppStep): JsonObject {
  const body = JSON.parse(step.request.body ?? '{}') as JsonObject;
  if (step.builder === undefined) {
    return body;
  }
  const entry = builderCase(step.builder);
  switch (entry.builder) {
    case 'search': {
      // The builder writes no namespace: the client's default fills it (R11).
      const built = search(entry.spec as { on: string; calls: Call[] }).toJson();
      return built;
    }
    case 'schema':
      return { ...body, schema: schema(entry.spec as Call[]) };
    case 'filter': {
      const spec = entry.spec as Call;
      return { ...body, where: filter(spec.op, args(spec)) };
    }
  }
  return body;
}

/** One app fixture, every step through the facade. Throws on the first step
 * that does not hold, naming it. */
export async function replayAppFixture(endpoint: string, name: string): Promise<void> {
  const fixture = readAppFixture(name);
  // Connect JSON, the encoding the scenario was recorded in; no retries, so an
  // answer a retry would have asked for again surfaces as the failure it is.
  const loams = new Loams({
    endpoint,
    namespace: 'app',
    maxRetries: 0,
    transportOptions: { useBinaryFormat: false },
  });
  for (const [at, step] of fixture.steps.entries()) {
    const where = `${name} step ${at} (${step.call})`;
    const dot = step.call.lastIndexOf('.');
    const moduleName = step.call.slice(0, dot);
    const callName = step.call.slice(dot + 1);
    // The corpus spells calls in `snake_case`; an older annotation may spell
    // its name in camel case (`getInstance`), so both are accepted.
    const camel = callName.replace(/_([a-z0-9])/g, (_m, c: string) => c.toUpperCase());
    const binding = MODULES.find((m) => m.name === moduleName)?.calls.find(
      (c) => c.protoName === callName || c.name === camel,
    );
    if (binding === undefined) {
      throw new Error(`${where}: the facade has no ${step.call}`);
    }
    const method = SERVICE_DESCRIPTORS[binding.service]?.methods.find((m) => m.name === binding.method);
    if (method === undefined) {
      throw new Error(`${where}: no descriptor for ${binding.rpc}`);
    }
    const request = fromJson(method.input, requestJson(step));
    const fn = (loams.modules[moduleName] as Record<string, unknown>)[binding.name] as (
      request: unknown,
      options: unknown,
    ) => Promise<unknown>;
    const headers = { 'loams-fixture-name': name, 'loams-fixture-step': String(at) };
    if (step.expect.status === 200) {
      let answer: unknown;
      try {
        answer = await fn(request, { headers });
      } catch (error) {
        throw new Error(`${where}: ${error instanceof Error ? error.message : String(error)}`);
      }
      const got = JSON.stringify(toJson(method.output, answer as never));
      const want = JSON.stringify(
        toJson(method.output, fromJson(method.output, step.response.body as JsonValue)),
      );
      if (got !== want) {
        throw new Error(`${where}: answered ${got.slice(0, 300)}, recorded ${want.slice(0, 300)}`);
      }
      continue;
    }
    const error = await fn(request, { headers }).then(
      () => undefined,
      (thrown: unknown) => thrown,
    );
    if (!(error instanceof LoamsError)) {
      throw new Error(`${where}: expected a refusal, got ${String(error)}`);
    }
    const recorded = step.response.body as { code?: string } | undefined;
    if (error.reason !== step.expect.reason) {
      throw new Error(`${where}: reason ${error.reason}, recorded ${step.expect.reason}`);
    }
    if (recorded?.code !== undefined && codeName(error.code) !== recorded.code) {
      throw new Error(`${where}: code ${codeName(error.code)}, recorded ${recorded.code}`);
    }
  }
}

/** Every app fixture; the names of the ones that held. */
export async function driveAppFixtures(endpoint: string): Promise<string[]> {
  const ran: string[] = [];
  for (const name of appFixtureNames()) {
    await replayAppFixture(endpoint, name);
    ran.push(name);
  }
  return ran;
}

const CODE_NAMES = [
  'ok',
  'canceled',
  'unknown',
  'invalid_argument',
  'deadline_exceeded',
  'not_found',
  'already_exists',
  'permission_denied',
  'resource_exhausted',
  'failed_precondition',
  'aborted',
  'out_of_range',
  'unimplemented',
  'internal',
  'unavailable',
  'data_loss',
  'unauthenticated',
];

/** A Connect code's wire name. */
function codeName(code: number): string {
  return CODE_NAMES[code] ?? String(code);
}
