// Stubs for the app-module tests: `DocumentService` bindings shaped as P0a's
// annotations generate them, and a transport that records request messages.

import type { CallBinding } from '../../src/gen/facade.js';

/** `DocumentService` bindings, as P0a's annotations will generate them. */
const binding = (name: string, method: string, retry: 'safe' | 'manual'): CallBinding => ({
  module: 'documents',
  name,
  protoName: name,
  method,
  rpc: `loams.collection.v1.DocumentService/${method}`,
  service: 'loams.collection.v1.DocumentService',
  package: 'loams.collection.v1',
  idempotency: retry === 'safe' ? 'no_side_effects' : 'none',
  retry,
  streaming: 'unary',
  pagination: null,
});
export const WRITE = binding('write', 'WriteDocuments', 'manual');
export const GET = binding('get', 'GetDocuments', 'safe');

/** A transport that records each request message and answers `answer`. */
export function messageTransport(answer: (method: string, input: Record<string, unknown>) => unknown) {
  const sent: { method: string; input: Record<string, unknown>; headers: Record<string, string> }[] = [];
  return {
    sent,
    transport: {
      unary(
        method: { name: string },
        _signal: unknown,
        _timeoutMs: number | undefined,
        header: HeadersInit | undefined,
        input: Record<string, unknown>,
      ) {
        sent.push({ method: method.name, input, headers: { ...(header as Record<string, string>) } });
        return Promise.resolve({ message: answer(method.name, input) }) as never;
      },
      stream() {
        throw new Error('no streams here');
      },
    },
  };
}
