// The composer's logic (SD1 Task 19): the `/`, `$` and `@` menus, attachment checks and
// the chunked upload, per-thread drafts in storage, and the send projection.

import { describe, expect, it, vi } from 'vite-plus/test';
import {
  type AttachmentUploader,
  checkAttachment,
  createComposerDraftStore,
  DRAFTS_STORAGE_KEY,
  type DraftStorage,
  fileMenuItems,
  promptForSend,
  readStoredDrafts,
  searchSkillMenu,
  searchSlashMenu,
  UPLOAD_CHUNK_BYTES,
  uploadAttachment,
} from '../src/composer';

const commands = [
  { name: 'review', description: 'Review the working tree' },
  { name: 'help', description: 'Show help' },
  { name: 'compact', description: 'Compact the conversation' },
];
const skills = [{ name: 'release-notes', description: 'Draft release notes', path: '/s/r.md' }];

describe('slash_command_menu_filters', () => {
  it('ranks name matches first and drops what does not match', () => {
    const ids = searchSlashMenu('rev', commands, skills).map((i) => i.id);
    expect(ids).toEqual(['command:review']);
    expect(searchSlashMenu('', commands, skills)).toHaveLength(4);
    expect(searchSlashMenu('notes', commands, skills).map((i) => i.id)).toEqual([
      'skill:release-notes',
    ]);
    expect(searchSlashMenu('conversation', commands, skills).map((i) => i.id)).toEqual([
      'command:compact',
    ]);
  });

  it('$ offers skills only', () => {
    expect(searchSkillMenu('', skills).map((i) => i.id)).toEqual(['skill:release-notes']);
    expect(searchSkillMenu('help', skills)).toEqual([]);
  });

  it('@ rows are files only, best score first', () => {
    const rows = fileMenuItems(
      [
        { path: 'src', name: 'src', kind: 'directory', score: 99 },
        { path: 'docs/readme.txt', name: 'readme.txt', kind: 'file', score: 3 },
        { path: 'README.md', name: 'README.md', kind: 'file', score: 9 },
      ],
      'read',
    );
    expect(rows.map((r) => (r.type === 'path' ? r.path : r.id))).toEqual([
      'README.md',
      'docs/readme.txt',
    ]);
  });
});

describe('heic_refused_with_message', () => {
  it('refuses HEIC by type or name, and other non-images, with a reason', () => {
    const heic = checkAttachment({ name: 'IMG.HEIC', type: '', size: 10 });
    expect(heic.ok).toBe(false);
    expect(!heic.ok && heic.reason).toMatch(/HEIC image.*PNG or JPEG/);
    expect(checkAttachment({ name: 'a.heif', type: 'image/heif', size: 1 }).ok).toBe(false);
    expect(checkAttachment({ name: 'a.pdf', type: 'application/pdf', size: 1 }).ok).toBe(false);
    expect(checkAttachment({ name: 'a.png', type: 'image/png', size: 1 }).ok).toBe(true);
    expect(checkAttachment({ name: 'a.png', type: 'image/png', size: 64 * 1024 ** 2 }).ok).toBe(
      false,
    );
  });
});

describe('upload_chunks_then_commits', () => {
  it('sends unpadded base64 chunks in order, then commits', async () => {
    const calls: string[] = [];
    const bytes = new Uint8Array(UPLOAD_CHUNK_BYTES + 7).map((_, i) => i % 256);
    const path = await uploadAttachment(
      new File([bytes], 'x.png', { type: 'image/png' }),
      {
        chunk: async (id, data, seq) => {
          calls.push(`chunk:${id}:${seq}:${data.length}`);
        },
        commit: async (id, name) => {
          calls.push(`commit:${id}:${name}`);
          return `/up/${name}`;
        },
      } satisfies AttachmentUploader,
      { uploadId: 'u1' },
    );
    expect(path).toBe('/up/x.png');
    expect(calls).toEqual([
      `chunk:u1:0:${(UPLOAD_CHUNK_BYTES / 3) * 4}`,
      'chunk:u1:1:12',
      'commit:u1:x.png',
    ]);
    expect(UPLOAD_CHUNK_BYTES % 3).toBe(0);
  });
});

function memoryStorage(): DraftStorage & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => {
      data.set(k, v);
    },
  };
}

describe('draft_survives_mode_switch (the store)', () => {
  it('a draft written by one page is read back by the next', () => {
    const storage = memoryStorage();
    const first = createComposerDraftStore(storage);
    first.getState().update('t1', { prompt: 'half a thought', harness: 'CODEX' });
    const second = createComposerDraftStore(storage);
    expect(second.getState().drafts.t1).toMatchObject({
      prompt: 'half a thought',
      harness: 'CODEX',
    });
  });

  it('clearSent keeps the choices and records the history; the stash round-trips', () => {
    const store = createComposerDraftStore(memoryStorage());
    store.getState().update('t1', { prompt: 'one', model: 'm' });
    store.getState().clearSent('t1', 'one');
    expect(store.getState().drafts.t1).toMatchObject({ prompt: '', model: 'm', history: ['one'] });
    store.getState().update('t1', { prompt: 'later' });
    store.getState().stash('t1');
    expect(store.getState().drafts.t1).toMatchObject({ prompt: '', stash: ['later'] });
    store.getState().unstash('t1', 0);
    expect(store.getState().drafts.t1).toMatchObject({ prompt: 'later', stash: [] });
  });
});

describe('draft_storage_failure_tolerated', () => {
  it('a throwing storage keeps drafts in memory', () => {
    const storage: DraftStorage = {
      getItem: vi.fn(() => {
        throw new Error('denied');
      }),
      setItem: vi.fn(() => {
        throw new Error('quota');
      }),
    };
    const store = createComposerDraftStore(storage);
    store.getState().update('t1', { prompt: 'kept' });
    expect(store.getState().drafts.t1?.prompt).toBe('kept');
  });

  it('corrupt or invalid stored drafts are dropped, valid ones kept', () => {
    const storage = memoryStorage();
    storage.data.set(DRAFTS_STORAGE_KEY, '{not json');
    expect(readStoredDrafts(storage)).toEqual({});
    storage.data.set(
      DRAFTS_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        drafts: { good: { prompt: 'ok' }, bad: { prompt: 7 }, worse: { attachments: [{}] } },
      }),
    );
    expect(Object.keys(readStoredDrafts(storage))).toEqual(['good']);
  });
});

describe('promptForSend', () => {
  it('a prompt without context is sent as written', () => {
    expect(promptForSend({ prompt: 'Fix [main.rs](src/main.rs) please ' })).toBe(
      'Fix [main.rs](src/main.rs) please',
    );
  });
});
