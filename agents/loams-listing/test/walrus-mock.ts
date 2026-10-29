// A stateful mock Walrus publisher and aggregator on loopback (node:http),
// after crates/loams-walrus/tests/support: PUT /v1/blobs stores, GET
// /v1/blobs/<id> reads.
import { createHash } from 'node:crypto';
import { createServer, type IncomingMessage, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';

/** A blob id the schema accepts: the bytes' SHA-256 as URL-safe base64 (43 characters). */
export function blobIdOf(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('base64url');
}

export interface MockWalrus {
  url: string;
  puts: number;
  reads: number;
  /** The Authorization header of each PUT. */
  auth: (string | undefined)[];
  /** The query of each PUT. */
  queries: URLSearchParams[];
  blobs: Map<string, Uint8Array>;
  /** Blob ids served with other bytes. */
  tampered: Set<string>;
  /** Answer this many 404s to each blob's first reads (a CDN's cached 404). */
  missFirst: number;
  /** The epoch new stores are registered in. */
  epoch: number;
  /** Answer every PUT with this instead. */
  answerWith?: { status: number; body: unknown };
  close(): Promise<void>;
}

async function body(req: IncomingMessage): Promise<Uint8Array> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return new Uint8Array(Buffer.concat(chunks));
}

export async function startMockWalrus(): Promise<MockWalrus> {
  const misses = new Map<string, number>();
  const endEpochs = new Map<string, number>();
  const state: Omit<MockWalrus, 'url' | 'close'> = {
    puts: 0,
    reads: 0,
    auth: [],
    queries: [],
    blobs: new Map(),
    tampered: new Set(),
    missFirst: 0,
    epoch: 100,
  };
  const server: Server = createServer((req, res) => {
    const url = new URL(req.url ?? '/', 'http://mock');
    void (async () => {
      if (req.method === 'PUT' && url.pathname === '/v1/blobs') {
        const bytes = await body(req);
        state.puts++;
        state.auth.push(req.headers.authorization);
        state.queries.push(url.searchParams);
        if (state.answerWith) {
          res.writeHead(state.answerWith.status, { 'content-type': 'application/json' });
          res.end(JSON.stringify(state.answerWith.body));
          return;
        }
        const id = blobIdOf(bytes);
        const epochs = Number(url.searchParams.get('epochs') ?? '1');
        let answer: unknown;
        if (state.blobs.has(id)) {
          answer = { alreadyCertified: { blobId: id, event: { txDigest: 'x', eventSeq: '0' }, endEpoch: endEpochs.get(id) } };
        } else {
          state.blobs.set(id, bytes);
          endEpochs.set(id, state.epoch + epochs);
          answer = {
            newlyCreated: {
              blobObject: { id: '0x01', registeredEpoch: state.epoch, blobId: id, size: bytes.length, storage: { id: '0x02', startEpoch: state.epoch, endEpoch: state.epoch + epochs, storageSize: bytes.length } },
              resourceOperation: { registerFromScratch: { encodedLength: bytes.length, epochsAhead: epochs } },
              cost: 1,
            },
          };
        }
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify(answer));
        return;
      }
      const m = /^\/v1\/blobs\/([A-Za-z0-9_-]+)$/.exec(url.pathname);
      if (req.method === 'GET' && m) {
        const id = m[1]!;
        state.reads++;
        const seen = misses.get(id) ?? 0;
        const bytes = state.blobs.get(id);
        if (!bytes || seen < state.missFirst) {
          misses.set(id, seen + 1);
          res.writeHead(404);
          res.end('not found');
          return;
        }
        res.writeHead(200, { 'content-type': 'application/octet-stream' });
        res.end(state.tampered.has(id) ? Buffer.from('tampered bytes') : Buffer.from(bytes));
        return;
      }
      res.writeHead(404);
      res.end();
    })();
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const { port } = server.address() as AddressInfo;
  return Object.assign(state, {
    url: `http://127.0.0.1:${port}`,
    close: () => new Promise<void>((r) => server.close(() => r())),
  });
}
