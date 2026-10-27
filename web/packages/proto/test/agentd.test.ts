// SD1 Task 4: `loams.agentd.v1` is generated into @loams/proto with a service
// descriptor per service of the Shared contracts, so connect-query can read
// every method over the agentd transport. `pnpm typecheck` compiling this file
// is `ts_descriptors_generated`; the assertions check what tsc cannot.

import { createClient, createRouterTransport } from '@connectrpc/connect';
import { describe, expect, it } from 'vite-plus/test';
import * as agentd from '../src/agentd.js';
import {
  ApprovalService,
  AttachmentService,
  DiffService,
  FactoryService,
  FileService,
  GitService,
  HarnessService,
  MarketplaceService,
  NotificationService,
  PreviewService,
  ProjectService,
  QueueService,
  ReviewService,
  RunGroupService,
  StudioService,
  TerminalService,
  ThreadService,
} from '../src/agentd.js';

/** SD1 Shared contracts: the services of `loams.agentd.v1`. */
const SERVICES = {
  StudioService,
  ProjectService,
  ThreadService,
  QueueService,
  HarnessService,
  GitService,
  DiffService,
  FileService,
  TerminalService,
  PreviewService,
  ReviewService,
  RunGroupService,
  ApprovalService,
  NotificationService,
  AttachmentService,
  FactoryService,
  // The Seed Exchange (MK1 Tasks 4b, 9).
  MarketplaceService,
};

describe('loams.agentd.v1', () => {
  it('ts_descriptors_generated: every service of the Shared contracts, with a client', () => {
    const transport = createRouterTransport(() => {});
    for (const [name, service] of Object.entries(SERVICES)) {
      expect(service.typeName).toBe(`loams.agentd.v1.${name}`);
      const methods = Object.keys(service.method);
      expect(methods.length).toBeGreaterThan(0);
      const client = createClient(service, transport) as Record<string, unknown>;
      for (const method of methods) expect(typeof client[method]).toBe('function');
    }
  });

  it('exports no other service', () => {
    const exported: unknown[] = Object.values(agentd);
    const services = exported.filter(
      (value): value is { kind: 'service'; typeName: string } =>
        typeof value === 'object' &&
        value !== null &&
        (value as { kind?: unknown }).kind === 'service',
    );
    expect(services.map((service) => service.typeName).sort()).toEqual(
      Object.keys(SERVICES)
        .map((name) => `loams.agentd.v1.${name}`)
        .sort(),
    );
  });

  it('streams the Watch methods and the terminal (server streaming)', () => {
    expect(ThreadService.method.watchTranscript.methodKind).toBe('server_streaming');
    expect(ThreadService.method.watchThreads.methodKind).toBe('server_streaming');
    expect(TerminalService.method.subscribeTerminal.methodKind).toBe('server_streaming');
    expect(HarnessService.method.listHarnesses.methodKind).toBe('unary');
  });
});
