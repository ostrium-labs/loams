import { describe, expect, it } from 'vite-plus/test';
import { plainError } from '../src/plain-error';

describe('plainError', () => {
  it('puts a bare gateway status in plain words', () => {
    for (const raw of [
      'HTTP 502',
      '502',
      'Bad Gateway',
      'HTTP 503 Service Unavailable',
      'Gateway Timeout',
    ]) {
      expect(plainError(raw)).toMatch(
        /^The server did not answer \(HTTP 50[234]\)\. Check that the engine is running/,
      );
    }
  });

  it('says a network failure is unreachable', () => {
    expect(plainError('Failed to fetch')).toMatch(/^The server could not be reached\./);
    expect(plainError('NetworkError when attempting to fetch resource.')).toMatch(
      /could not be reached/,
    );
  });

  it('reads a long run of spaces in linear time', () => {
    const started = performance.now();
    expect(plainError(`502${' '.repeat(50_000)}!`)).toMatch(/^502 /);
    expect(performance.now() - started).toBeLessThan(1_000);
  });

  it("keeps the server's own sentence", () => {
    expect(plainError('namespace "e2e" not found')).toBe('namespace "e2e" not found');
    expect(plainError('HTTP 404')).toBe('HTTP 404');
  });
});
