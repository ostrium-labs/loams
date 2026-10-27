// AP0 Task 6: every app service yields a typed Connect client, and a
// round trip through an in-memory router keeps the watch-stream shape of
// AP0 Ruling 3 (snapshot, then changes, then heartbeats, each with a cursor).

import {
  create,
  fromJson,
  type JsonValue,
  type MessageInitShape,
  toJson,
} from '@bufbuild/protobuf';
import { Code, ConnectError, createClient, createRouterTransport } from '@connectrpc/connect';
import { describe, expect, it } from 'vite-plus/test';
import {
  ApprovalSchema,
  ApprovalService,
  ApprovalState,
  DecisionKind,
  type WatchApprovalsResponseSchema,
} from '../src/gen/loams/approvals/v1/approvals_pb.js';
import { DeviceService } from '../src/gen/loams/devices/v1/devices_pb.js';
import {
  GraphAdminService,
  GraphService,
  ValueSchema as GraphValueSchema,
} from '../src/gen/loams/graph/v1/graph_pb.js';
import {
  ExecuteQueryRequestSchema,
  ExecuteQueryResponseSchema,
  HouseService,
  ResultFormat,
} from '../src/gen/loams/house/v1/house_pb.js';
import { Edition, InstanceService } from '../src/gen/loams/instance/v1/instance_pb.js';
import { NotificationService } from '../src/gen/loams/notifications/v1/notifications_pb.js';
import { OperationsService } from '../src/gen/loams/operations/v1/operations_pb.js';
import { APP_API_PACKAGES } from '../src/index.js';

describe('@loams/proto', () => {
  it('builds a client for every app service', () => {
    const transport = createRouterTransport(() => {});
    for (const service of [
      InstanceService,
      DeviceService,
      ApprovalService,
      OperationsService,
      NotificationService,
    ]) {
      const client = createClient(service, transport);
      expect(Object.keys(service.method).length).toBeGreaterThan(0);
      for (const name of Object.keys(service.method)) {
        expect(typeof (client as Record<string, unknown>)[name]).toBe('function');
      }
    }
  });

  it('names every service package in APP_API_PACKAGES', () => {
    const packages = [
      InstanceService,
      DeviceService,
      ApprovalService,
      OperationsService,
      NotificationService,
    ].map((s) => s.typeName.split('.').slice(0, -1).join('.'));
    expect([...APP_API_PACKAGES].sort()).toEqual(packages.sort());
  });

  it('marks idempotent reads as NO_SIDE_EFFECTS so Connect may use GET', () => {
    expect(InstanceService.method.getInstance.idempotency).toBe(1);
    expect(ApprovalService.method.listApprovals.idempotency).toBe(1);
    expect(ApprovalService.method.decideApproval.idempotency).toBe(0);
  });

  it('round-trips unary calls and a watch stream through a router', async () => {
    const pending = create(ApprovalSchema, {
      id: 'apr_01',
      revision: 3n,
      kind: 'collection.drop',
      summary: 'Drop collection docs',
      state: ApprovalState.PENDING,
    });
    const transport = createRouterTransport(({ service }) => {
      service(InstanceService, {
        getInstance: () => ({
          instanceId: '01J9',
          edition: Edition.OSS,
          apiVersions: ['loams.approvals.v1'],
        }),
        whoAmI: () => {
          throw new ConnectError('not signed in', Code.Unauthenticated);
        },
      });
      service(ApprovalService, {
        listApprovals: () => ({ approvals: [pending] }),
        getApproval: () => ({ approval: pending }),
        async *watchApprovals(): AsyncGenerator<
          MessageInitShape<typeof WatchApprovalsResponseSchema>
        > {
          yield { event: { case: 'snapshot', value: { approvals: [pending] } }, cursor: 'c1' };
          yield { event: { case: 'remove', value: 'apr_01' }, cursor: 'c2' };
          yield { event: { case: 'heartbeat', value: {} }, cursor: 'c2' };
        },
        decideApproval: (req) => {
          if (req.revision !== pending.revision) {
            throw new ConnectError('stale', Code.FailedPrecondition);
          }
          return { approval: { ...pending, state: ApprovalState.APPROVED } };
        },
      });
    });

    const instance = createClient(InstanceService, transport);
    const info = await instance.getInstance({});
    expect(info.edition).toBe(Edition.OSS);
    await expect(instance.whoAmI({})).rejects.toMatchObject({ code: Code.Unauthenticated });

    const approvals = createClient(ApprovalService, transport);
    const events: string[] = [];
    for await (const res of approvals.watchApprovals({})) {
      events.push(`${res.event.case}@${res.cursor}`);
    }
    expect(events).toEqual(['snapshot@c1', 'remove@c2', 'heartbeat@c2']);

    await expect(
      approvals.decideApproval({
        approvalId: 'apr_01',
        revision: 2n,
        decision: DecisionKind.APPROVE,
      }),
    ).rejects.toMatchObject({ code: Code.FailedPrecondition });
    const decided = await approvals.decideApproval({
      approvalId: 'apr_01',
      revision: 3n,
      decision: DecisionKind.APPROVE,
    });
    expect(decided.approval?.state).toBe(ApprovalState.APPROVED);
  });

  it('generates loams.graph.v1 and reads the desktop value fixtures', async () => {
    const transport = createRouterTransport(() => {});
    for (const service of [GraphAdminService, GraphService]) {
      const client = createClient(service, transport);
      for (const name of Object.keys(service.method)) {
        expect(typeof (client as Record<string, unknown>)[name]).toBe('function');
      }
    }
    expect(GraphService.method.executeStream.methodKind).toBe('server_streaming');
    // conformance/graph/desktop/values.json (GR1 Tasks 2 and 7): every case decodes with
    // the generated Value and encodes back unchanged.
    const fsId = 'node:fs';
    const { readFileSync } = (await import(/* @vite-ignore */ fsId)) as {
      readFileSync(url: URL, enc: 'utf8'): string;
    };
    const fixture = JSON.parse(
      readFileSync(
        new URL('../../../../conformance/graph/desktop/values.json', import.meta.url),
        'utf8',
      ),
    ) as { cases: { name: string; value: JsonValue }[] };
    expect(fixture.cases.length).toBeGreaterThan(10);
    for (const c of fixture.cases) {
      expect(toJson(GraphValueSchema, fromJson(GraphValueSchema, c.value)), c.name).toEqual(
        c.value,
      );
    }
  });

  it('generates loams.house.v1 with streaming results and an absent read_only', async () => {
    expect(HouseService.typeName).toBe('loams.house.v1.HouseService');
    expect(HouseService.method.executeQuery.methodKind).toBe('server_streaming');
    expect(HouseService.method.previewTable.methodKind).toBe('server_streaming');
    expect(HouseService.method.listTables.idempotency).toBe(1);
    expect(HouseService.method.executeQuery.idempotency).toBe(0);
    // `read_only` is `optional`: absent means true on the server (§49 §18.2), so a
    // client that never sets it can only read.
    const request = create(ExecuteQueryRequestSchema, { sql: 'SELECT 1' });
    expect(request.readOnly).toBeUndefined();
    expect(request.format).toBe(ResultFormat.UNSPECIFIED);

    const transport = createRouterTransport(({ service }) => {
      service(HouseService, {
        async *executeQuery(req) {
          yield create(ExecuteQueryResponseSchema, {
            frame: {
              case: 'header',
              value: { queryId: 'q1', columns: [{ name: 'x', clickhouseType: 'UInt8' }] },
            },
          });
          yield create(ExecuteQueryResponseSchema, {
            frame: {
              case: 'batch',
              value: {
                data: { case: 'rowsJson', value: new TextEncoder().encode('[1]\n') },
                rows: 1n,
              },
            },
          });
          yield create(ExecuteQueryResponseSchema, {
            frame: {
              case: 'summary',
              value: { queryId: 'q1', resultRows: 1n, truncated: req.maxRows === 1 },
            },
          });
        },
      });
    });
    const client = createClient(HouseService, transport);
    const cases: string[] = [];
    for await (const frame of client.executeQuery({
      sql: 'SELECT 1',
      maxRows: 1,
      readOnly: true,
    })) {
      cases.push(frame.frame.case ?? '');
      if (frame.frame.case === 'summary') expect(frame.frame.value.truncated).toBe(true);
    }
    expect(cases).toEqual(['header', 'batch', 'summary']);
  });
});
