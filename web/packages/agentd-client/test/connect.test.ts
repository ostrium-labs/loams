// The agentd Connect client (plan SD1 Task 6): the desktop transport goes through
// Electron main's protocol route, and a Connect transcript frame folds to the
// same state as its ndjson twin.

import { create, fromBinary } from '@bufbuild/protobuf';
import { createClient } from '@connectrpc/connect';
import { HarnessService, TranscriptUpdateSchema } from '@loams/proto/agentd';
import { describe, expect, it } from 'vite-plus/test';
import {
  AGENTD_GIT_WRITE_METHODS,
  createAgentdTransport,
  DESKTOP_AGENTD_BASE_URL,
  fromConnectTranscript,
  isGitWriteMethod,
  reduceAnyTranscript,
} from '../src/connect';
import {
  fixture_TranscriptUpdate_delta,
  fixture_TranscriptUpdate_reset,
} from '../src/fixtures.gen';
import { EMPTY_TRANSCRIPT, reduceTranscript, type TranscriptState } from '../src/transcript';
import connectFixtures from './transcript.connect.gen.json';

function hex(text: string): Uint8Array {
  return Uint8Array.from(text.match(/../g) ?? [], (b) => Number.parseInt(b, 16));
}

const connect = (name: keyof typeof connectFixtures) =>
  fromBinary(TranscriptUpdateSchema, hex(connectFixtures[name]));

describe('the desktop transport', () => {
  it('transport_desktop_uses_protocol_base', async () => {
    const seen: { url: string; headers: Headers }[] = [];
    const fetch: typeof globalThis.fetch = async (input, init) => {
      seen.push({ url: String(input), headers: new Headers(init?.headers) });
      return new Response(JSON.stringify({ code: 'unavailable', message: 'agentd_unavailable' }), {
        status: 503,
        headers: { 'content-type': 'application/json' },
      });
    };
    const client = createClient(HarnessService, createAgentdTransport({ kind: 'desktop', fetch }));
    await expect(client.listHarnesses({})).rejects.toThrow();
    expect(DESKTOP_AGENTD_BASE_URL).toBe('loams-app://console/');
    expect(seen).toHaveLength(1);
    expect(seen[0]?.url).toBe('loams-app://console/loams.agentd.v1.HarnessService/ListHarnesses');
    // Binary Connect, and no credential of the renderer's own: main adds the token.
    expect(seen[0]?.headers.get('content-type')).toBe('application/proto');
    expect(seen[0]?.headers.get('authorization')).toBeNull();
  });
});

describe('the remote transport (SD1 Task 7, ruling T7-9)', () => {
  it('createAgentdTransport_remote_calls_the_host_with_the_bearer', async () => {
    const seen: { url: string; headers: Headers; credentials?: RequestCredentials }[] = [];
    const fetch: typeof globalThis.fetch = async (input, init) => {
      seen.push({
        url: String(input),
        headers: new Headers(init?.headers),
        credentials: init?.credentials,
      });
      return new Response(JSON.stringify({ code: 'unauthenticated', message: 'no' }), {
        status: 401,
        headers: { 'content-type': 'application/json' },
      });
    };
    const transport = createAgentdTransport({
      kind: 'remote',
      baseUrl: 'https://devbox.example.ts.net:8443/',
      getToken: () => 'access-token',
      fetch,
    });
    const client = createClient(HarnessService, transport);
    await expect(client.listHarnesses({})).rejects.toThrow();
    expect(seen[0]?.url).toBe(
      'https://devbox.example.ts.net:8443/loams.agentd.v1.HarnessService/ListHarnesses',
    );
    expect(seen[0]?.headers.get('authorization')).toBe('Bearer access-token');
    expect(seen[0]?.credentials).toBe('omit');
    // A host that is not https is refused before any request.
    expect(() =>
      createAgentdTransport({
        kind: 'remote',
        baseUrl: 'http://evil.example',
        getToken: () => 't',
      }),
    ).toThrow(/https/);
  });
});

describe('the git-write methods', () => {
  it('are the generated set', () => {
    expect(AGENTD_GIT_WRITE_METHODS.has('/loams.agentd.v1.GitService/PushBranch')).toBe(true);
    expect(isGitWriteMethod('loams.agentd.v1.GitService', 'CommitWorktree')).toBe(true);
    expect(isGitWriteMethod('loams.agentd.v1.DiffService', 'Discard')).toBe(true);
    expect(isGitWriteMethod('loams.agentd.v1.GitService', 'ListBranches')).toBe(false);
  });
});

describe('the transcript', () => {
  it('reducer_accepts_connect_updates', () => {
    // Each fixture, mapped by the daemon to loams.agentd.v1 and encoded, maps
    // back to the ndjson frame.
    expect(fromConnectTranscript(connect('TranscriptUpdate.reset'))).toEqual(
      fixture_TranscriptUpdate_reset,
    );
    expect(fromConnectTranscript(connect('TranscriptUpdate.delta'))).toEqual(
      fixture_TranscriptUpdate_delta,
    );
    // And the frames fold to the same state over either wire.
    let nd: TranscriptState = EMPTY_TRANSCRIPT;
    let cx: TranscriptState = EMPTY_TRANSCRIPT;
    for (const name of ['TranscriptUpdate.reset', 'TranscriptUpdate.delta'] as const) {
      const ndjson =
        name === 'TranscriptUpdate.reset'
          ? fixture_TranscriptUpdate_reset
          : fixture_TranscriptUpdate_delta;
      nd = reduceTranscript(nd, ndjson);
      cx = reduceAnyTranscript(cx, connect(name));
      expect(cx).toEqual(nd);
    }
    expect(nd.desync).toBeNull();
    expect(nd.entries.map((e) => e.id)).toEqual(['msg-u1', 'msg-1']);
    const first = nd.entries[1]?.parts[0];
    expect(first?.kind === 'text' && first.text).toBe('Building now. Done.');
  });

  it('a delta that does not apply leaves the entries and says so', () => {
    const state = reduceTranscript(EMPTY_TRANSCRIPT, fixture_TranscriptUpdate_delta);
    expect(state.entries).toEqual([]);
    expect(state.desync).toMatch(/missing anchor msg-u1/);
    // A reset recovers.
    expect(reduceTranscript(state, fixture_TranscriptUpdate_reset).desync).toBeNull();
  });

  it('an empty Connect frame is refused', () => {
    expect(() => fromConnectTranscript(create(TranscriptUpdateSchema))).toThrow(/no frame/);
  });
});
