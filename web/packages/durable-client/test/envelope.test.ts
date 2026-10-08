import { describe, expect, it } from 'vitest';
import { ENVELOPE_VERSION, EnvelopeError, envelope } from '../src/index.js';

const reply = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

describe('envelope', () => {
  it('posts_the_envelope_and_returns_data', async () => {
    let seen: { url: string; init?: RequestInit } | undefined;
    const send = envelope(async (url, init) => {
      seen = { url: String(url), init };
      return reply({
        kind: 'promise.get',
        head: { corrId: 'c', status: 200 },
        data: { promise: { id: 'p' } },
      });
    }, 'http://x');
    expect(await send('promise.get', { id: 'p' })).toEqual({ promise: { id: 'p' } });
    expect(seen?.url).toBe('http://x/durable/');
    expect(seen?.init?.method).toBe('POST');
    expect(seen?.init?.credentials).toBe('include');
    const body = JSON.parse(String(seen?.init?.body));
    expect(body.kind).toBe('promise.get');
    expect(body.head.version).toBe(ENVELOPE_VERSION);
    expect(typeof body.head.corrId).toBe('string');
    expect(body.data).toEqual({ id: 'p' });
  });

  it('maps_failures_to_EnvelopeError', async () => {
    const run = (res: Response) => envelope(async () => res)('k', {});
    const a = await run(reply({ head: { status: 404 }, data: 'not found' })).catch((e) => e);
    expect(a).toBeInstanceOf(EnvelopeError);
    expect(a.status).toBe(404);
    expect(a.message).toBe('not found');
    // The proxy's own refusal is {code, message}, not an envelope.
    const b = await run(reply({ code: 'no_engine', message: 'engine stopped' }, 503)).catch(
      (e) => e,
    );
    expect(b.status).toBe(503);
    expect(b.message).toBe('engine stopped');
    // Not JSON at all.
    const c = await run(new Response('<html>', { status: 502 })).catch((e) => e);
    expect(c.status).toBe(502);
    expect(c.message).toContain('HTTP 502');
  });
});
