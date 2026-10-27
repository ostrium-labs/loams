// `transportOptions.headers` reaches the wire, and `loams.live.admin` exists.
//
// B17 (0.1.0 release e2e): neither connect-web's `createConnectTransport` nor
// connect-node's `createGrpcTransport` has a `headers` option, so the headers
// `createLoamsTransport` passed were dropped without a word. A Live deploy meant
// for another app (`loams-live-app: shop`) went out without the header and
// replaced the default app's deployment. These tests record what a real server
// receives, on both transports, for a unary call and a server stream.
//
// The same release found `loams.live.admin` documented (design §44 §7.2, D689)
// but undefined: the module was only reachable as `loams.modules['live.admin']`.

import { createServer, type IncomingHttpHeaders, type Server } from 'node:http';
import {
  createServer as createHttp2Server,
  type Http2Server,
  type Http2Session,
} from 'node:http2';
import type { AddressInfo } from 'node:net';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { Loams } from '../src/loams.js';
import { createNodeTransport } from '../src/node.js';
import { createLoamsTransport } from '../src/runtime/transports.js';

/** A server that records every request's path and headers and refuses it
 * with a Connect `unimplemented` (HTTP/1.1) or gRPC status 12 (HTTP/2). The
 * call fails; the test only cares what arrived. */
interface Recorder {
  readonly endpoint: string;
  readonly seen: { path: string; headers: IncomingHttpHeaders }[];
  close(): Promise<void>;
}

let open: Recorder | undefined;

afterEach(async () => {
  await open?.close();
  open = undefined;
});

async function recordHttp1(): Promise<Recorder> {
  const seen: Recorder['seen'] = [];
  const server: Server = createServer((request, response) => {
    seen.push({ path: request.url ?? '', headers: request.headers });
    request.resume();
    request.on('end', () => {
      response.writeHead(404, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ code: 'unimplemented', message: 'recorded' }));
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  open = {
    endpoint: `http://127.0.0.1:${port}`,
    seen,
    // Keep-alive connections would hold `close` open past the hook's timeout.
    close: () =>
      new Promise((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      }),
  };
  return open;
}

async function recordHttp2(): Promise<Recorder> {
  const seen: Recorder['seen'] = [];
  const server: Http2Server = createHttp2Server();
  // The client keeps its HTTP/2 session open; `close` waits for every session.
  const sessions = new Set<Http2Session>();
  server.on('session', (session) => {
    sessions.add(session);
    session.on('close', () => sessions.delete(session));
  });
  server.on('stream', (stream, headers) => {
    seen.push({ path: String(headers[':path']), headers: headers as IncomingHttpHeaders });
    stream.resume();
    stream.respond(
      { ':status': 200, 'content-type': 'application/grpc', 'grpc-status': '12' },
      { endStream: true },
    );
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  open = {
    endpoint: `http://127.0.0.1:${port}`,
    seen,
    close: () =>
      new Promise((resolve) => {
        server.close(() => resolve());
        for (const session of sessions) {
          session.destroy();
        }
      }),
  };
  return open;
}

/** Drains a stream, swallowing the refusal the recorder answers with. */
async function drain(stream: AsyncIterable<unknown>): Promise<void> {
  try {
    for await (const _ of stream) {
      // nothing arrives
    }
  } catch {
    // the recorder refuses every call
  }
}

const APP = { 'loams-live-app': 'shop', 'x-gateway': 'edge-1' };

describe('transportOptions.headers', () => {
  it('reaches the wire on a unary call over the default Connect transport', async () => {
    const server = await recordHttp1();
    const loams = new Loams({
      endpoint: server.endpoint,
      transportOptions: { headers: APP },
      maxRetries: 0,
    });
    await expect(loams.live.admin.deploy({})).rejects.toThrow();
    expect(server.seen).toHaveLength(1);
    expect(server.seen[0]!.path).toBe('/loams.live.v1.LiveAdminService/Deploy');
    expect(server.seen[0]!.headers['loams-live-app']).toBe('shop');
    expect(server.seen[0]!.headers['x-gateway']).toBe('edge-1');
  });

  it('reaches the wire on a server stream over the default Connect transport', async () => {
    const server = await recordHttp1();
    const loams = new Loams({
      endpoint: server.endpoint,
      transportOptions: { headers: APP },
      maxRetries: 0,
    });
    await drain(loams.live.admin.tailLogs({}));
    expect(server.seen.length).toBeGreaterThanOrEqual(1);
    expect(server.seen[0]!.path).toBe('/loams.live.v1.LiveAdminService/TailLogs');
    expect(server.seen[0]!.headers['loams-live-app']).toBe('shop');
  });

  it('does not overwrite the credential or a per-call header', async () => {
    const server = await recordHttp1();
    const loams = new Loams({
      endpoint: server.endpoint,
      auth: 'lk_test_key',
      transportOptions: {
        headers: { authorization: 'Bearer static', 'loams-live-app': 'default' },
      },
      maxRetries: 0,
    });
    await expect(
      loams.live.admin.deploy({}, { headers: { 'loams-live-app': 'shop' } }),
    ).rejects.toThrow();
    expect(server.seen[0]!.headers.authorization).toBe('Bearer lk_test_key');
    expect(server.seen[0]!.headers['loams-live-app']).toBe('shop');
  });

  it("runs alongside the caller's own interceptors", async () => {
    const server = await recordHttp1();
    const transport = createLoamsTransport({
      baseUrl: server.endpoint,
      headers: APP,
      interceptors: [
        (next) => (request) => {
          request.header.set('x-interceptor', request.header.get('loams-live-app') ?? 'missing');
          return next(request);
        },
      ],
    });
    const loams = new Loams({ endpoint: server.endpoint, transport, maxRetries: 0 });
    await expect(loams.instance.getInstance({})).rejects.toThrow();
    expect(server.seen[0]!.headers['loams-live-app']).toBe('shop');
    expect(server.seen[0]!.headers['x-interceptor']).toBe('shop');
  });

  it('reaches the wire over the Node HTTP/2 gRPC transport, unary and streaming', async () => {
    const server = await recordHttp2();
    const loams = new Loams({
      endpoint: server.endpoint,
      transport: createNodeTransport({ baseUrl: server.endpoint, headers: APP }),
      maxRetries: 0,
    });
    await expect(loams.live.admin.deploy({})).rejects.toThrow();
    await drain(loams.live.admin.tailLogs({}));
    expect(server.seen.map((entry) => entry.path)).toEqual([
      '/loams.live.v1.LiveAdminService/Deploy',
      '/loams.live.v1.LiveAdminService/TailLogs',
    ]);
    for (const entry of server.seen) {
      expect(entry.headers['loams-live-app']).toBe('shop');
      expect(entry.headers['x-gateway']).toBe('edge-1');
    }
  });
});

describe('the module tree', () => {
  const loams = new Loams({ endpoint: 'http://127.0.0.1:1' });

  it('has loams.live.admin, the same object as modules["live.admin"]', () => {
    expect(loams.live.admin).toBeDefined();
    expect(loams.live.admin).toBe(loams.modules['live.admin']);
    expect(loams.live.admin.module).toBe('live.admin');
    expect(loams.live.admin.service).toBe('loams.live.v1.LiveAdminService');
    expect(typeof loams.live.admin.deploy).toBe('function');
    expect(typeof loams.live.admin.listDeployments).toBe('function');
    // `loams.live` keeps its own calls.
    expect(typeof loams.live.watch).toBe('function');
  });

  it('has loams.admin.* and loams.auth', () => {
    expect(loams.admin.org).toBe(loams.modules['admin.org']);
    expect(loams.admin.projects).toBe(loams.modules['admin.projects']);
    expect(loams.admin.agents).toBe(loams.modules['admin.agents']);
    expect(loams.admin.keys).toBe(loams.modules['admin.keys']);
    expect(loams.admin.audit).toBe(loams.modules['admin.audit']);
    expect(loams.auth).toBe(loams.modules.auth);
  });
});
