// TypeScript → Rust (design §50 §8.3). The typed builders below write
// `requests.json`; the Rust test `ts_requests_parse` parses every entry into
// its method's params type. This test fails when the committed file
// differs; regenerate it with `UPDATE_REQUESTS=1 pnpm --filter
// @loams/agentd-client test`.

import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { METHODS } from '../src/gen/methods';
import { type Request, request } from '../src/requests';

const chatId = 'chat-1';
const target = { chatId };
const run = {
  prompt: 'Fix the build',
  model: 'claude-sonnet',
  reasoning: 'high',
  modelOptions: {},
  cwd: '/home/dev/project',
  sandbox: 'workspace-write',
  autoApprove: false,
  resume: null,
} as const;

const REQUESTS: Request[] = [
  request('Hello', { client: 'electron-main', clientVersion: '0.2.1', protocol: 1 }),
  request('Shutdown', {}),
  request('WatchPreviews', { chatId }),
  request('ListHarnesses', {}),
  request('CancelInstall', { harness: 'codex' }),
  request('InstallHarness', { harness: 'codex' }),
  request('GetTitleSettings', {}),
  request('SetTitleSettings', { harness: 'claude-code', model: null }),
  request('SetHarnessEnabled', { harness: 'pi', enabled: false }),
  request('ListModels', { harness: 'codex', force: false }),
  request('ListSkills', { harness: 'claude-code', chatId }),
  request('ListCommands', { harness: 'claude-code', spaceId: 'space-1' }),
  request('QueueCommand', {
    chatId,
    command: { kind: 'run', request: { ...run, harness: 'claude-code' }, messageId: 'msg-u1' },
  }),
  request('QueueCommand', {
    chatId,
    command: { kind: 'steer', prompt: 'Also run the tests', messageId: null },
  }),
  request('QueueCommand', { chatId, command: { kind: 'interrupt' } }),
  request('QueueCommand', {
    chatId,
    command: {
      kind: 'respondInput',
      requestId: 'req-1',
      answers: [{ questionId: 'q-1', labels: ['main'] }],
    },
  }),
  request('RetryDelivery', { chatId }),
  request('TakeProjectActionSetup', { chatId, commandId: 'cmd-1' }),
  request('ForkSideChat', { chatId: 'chat-2', sourceChatId: chatId }),
  request('ListCheckpoints', { chatId }),
  request('RestoreCheckpoint', {
    chatId,
    turnId: 'msg-1',
    expectedChecksum: 'sum-1',
    scope: 'worktree+transcript',
  }),
  request('GetThreadUsage', { chatId }),
  request('WatchNotifications', {}),
  request('WatchDocMessages', { chatId, openingTail: true }),
  request('WatchQueue', { chatId }),
  request('QueueMessage', { chatId, text: 'Then update the docs' }),
  request('UpdateQueuedMessage', { chatId, id: 'queued-1', text: 'Then update the README' }),
  request('BeginQueuedMessageEdit', {
    chatId,
    id: 'queued-1',
    editorDeviceId: 'device-1',
    editorInstanceId: 'window-1',
  }),
  request('RenewQueuedMessageEdit', { chatId, id: 'queued-1', leaseId: 'lease-1' }),
  request('FinishQueuedMessageEdit', {
    chatId,
    id: 'queued-1',
    leaseId: 'lease-1',
    action: 'releaseUnchanged',
  }),
  request('MoveQueuedMessage', { chatId, id: 'queued-1', toIndex: 0 }),
  request('RemoveQueuedMessage', { chatId, id: 'queued-1' }),
  request('SendQueuedMessageNow', { chatId, id: 'queued-1' }),
  request('SteerQueuedMessageNow', { chatId, id: 'queued-1' }),
  request('WatchChats', {}),
  request('WatchSidebarPreferences', {}),
  request('WatchSessions', {}),
  request('WatchSpaces', {}),
  request('Mutate', { op: 'createChat', chatId: 'chat-2', spaceId: 'space-1' }),
  request('Mutate', {
    op: 'createSpace',
    spaceId: 'space-2',
    deviceId: 'device-1',
    path: '/home/dev/other',
    gitDetected: true,
  }),
  request('Mutate', { op: 'renameChat', chatId, title: 'Fix the linker' }),
  request('Mutate', { op: 'setChatArchived', chatId, archived: true }),
  request('Mutate', {
    op: 'changeSidebarPin',
    change: { action: 'pin', sessionId: chatId, after: null, before: null },
  }),
  request('Mutate', {
    op: 'setChatConfig',
    chatId,
    config: {
      harness: 'codex',
      model: null,
      reasoning: null,
      modelOptions: {},
      sandbox: 'read-only',
    },
  }),
  request('Mutate', { op: 'markChatSeen', chatId }),
  request('LocalDevice', {}),
  request('EngineInfo', {}),
  request('EngineReady', {}),
  request('ListRepos', {}),
  request('AddRepo', { path: '/home/dev/project' }),
  request('CloneRepo', { url: 'https://github.com/example/project.git' }),
  request('CreateRepo', { name: 'project' }),
  request('ListBranches', { repoPath: '/home/dev/project' }),
  request('ListRefs', { repoPath: '/home/dev/project' }),
  request('ListGitHistory', { cwd: '/home/dev/project', cursor: 0, limit: 100 }),
  request('SearchGitHistory', { cwd: '/home/dev/project', query: 'build', cursor: 0, limit: 50 }),
  request('ResolveGitAvatars', {
    cwd: '/home/dev/project',
    authors: [{ sha: '0123abcd', email: 'dev@example.com' }],
    cursor: 0,
    limit: 100,
  }),
  request('FetchAll', { repoPath: '/home/dev/project' }),
  request('SwitchRef', { repoPath: '/home/dev/project', refName: 'origin/dev' }),
  request('ListFolders', { path: '/home/dev' }),
  request('ListDrives', {}),
  request('SearchFiles', { query: 'main', chatId }),
  request('ListWorkspaceDirectory', { ...target, directory: 'src', includeIgnored: false }),
  request('SearchWorkspaceFiles', { ...target, query: 'main', includeIgnored: false }),
  request('ReadWorkspaceImage', {
    ...target,
    path: 'docs/shot.png',
    expectedCheckoutId: 'checkout-1',
    offset: 0,
    expectedContentHash: null,
  }),
  request('ReadWorkspaceFile', { ...target, path: 'src/main.rs' }),
  request('MoveWorkspaceEntry', {
    ...target,
    operationId: 'op-1',
    expectedCheckoutId: 'checkout-1',
    sourcePath: 'src/main.rs',
    destinationPath: 'src/lib.rs',
    expectedSourceRevision: 'rev-1',
    expectedKind: 'file',
  }),
  request('DeleteWorkspaceEntry', {
    ...target,
    operationId: 'op-2',
    expectedCheckoutId: 'checkout-1',
    path: 'target',
    expectedSourceRevision: 'rev-2',
    expectedKind: 'directory',
    recursive: true,
  }),
  request('WriteWorkspaceFile', {
    ...target,
    expectedCheckoutId: 'checkout-1',
    path: 'src/main.rs',
    text: 'fn main() {}\n',
    expectedContentHash: 'h-1',
    encoding: 'utf8',
    lineEnding: 'lf',
  }),
  request('WatchWorkspaceFiles', target),
  request('CreateWorktree', {
    repoPath: '/home/dev/project',
    branch: 'fix-linker',
    spaceId: 'space-1',
  }),
  request('DeleteWorktree', {
    repoPath: '/home/dev/project',
    worktreePath: '/home/dev/project-wt/fix-linker',
  }),
  request('ListProjectActions', { spaceId: 'space-1' }),
  request('UpsertProjectAction', {
    spaceId: 'space-1',
    action: { name: 'Test', command: 'cargo test', icon: 'test', runOnWorktreeCreate: false },
  }),
  request('DeleteProjectAction', { spaceId: 'space-1', actionId: 'action-1' }),
  request('RunProjectAction', {
    spaceId: 'space-1',
    chatId,
    actionId: 'action-1',
    cols: 120,
    rows: 32,
  }),
  request('OpenTerminal', { chatId, cols: 120, rows: 32 }),
  request('SubscribeTerminal', { terminalId: 'term-1', afterSeq: 10 }),
  request('WriteTerminal', { terminalId: 'term-1', data: 'bHMK' }),
  request('ResizeTerminal', { terminalId: 'term-1', cols: 100, rows: 30 }),
  request('CloseTerminal', { terminalId: 'term-1' }),
  request('WatchCheckoutDiffs', {}),
  request('WatchWorkspaceGitStatus', target),
  request('WatchCheckoutChangeRequest', { cwd: '/home/dev/project', branch: 'fix-linker' }),
  request('GetCheckoutDiff', { cwd: '/home/dev/project', mode: 'branch', baseRef: 'main' }),
  request('DiscardWorkingTree', {
    chatId,
    checkoutId: 'checkout-1',
    expectedChecksum: 'sum-1',
  }),
  request('DiscardPaths', {
    checkoutId: 'checkout-1',
    paths: ['src/main.rs'],
    expectedChecksum: 'sum-1',
  }),
  request('ListWorktrees', { repoPath: '/home/dev/project' }),
  request('CommitWorktree', {
    checkoutId: 'checkout-1',
    cwd: '/home/dev/project',
    message: 'Fix the linker',
    paths: ['src/main.rs'],
    all: false,
    amend: false,
    signoff: true,
    expectedChecksum: 'sum-1',
  }),
  request('PushBranch', {
    cwd: '/home/dev/project',
    remote: 'origin',
    branch: 'fix-linker',
    setUpstream: true,
    forceWithLease: false,
  }),
  request('CreateChangeRequest', {
    cwd: '/home/dev/project',
    provider: 'forgejo',
    base: 'main',
    head: 'fix-linker',
    title: 'Fix the linker',
    body: 'Links again.',
    draft: false,
    linkIssue: '42',
  }),
  request('MergeBranch', {
    repoPath: '/home/dev/project',
    source: 'loams-desktop/fix-linker',
    target: 'main',
    strategy: 'squash',
    message: 'Fix the linker',
    deleteSource: true,
    removeWorktree: true,
    expectedSourceHead: '4b825dc642cb6eb9a060e54bf8d69288fbee4904',
  }),
  request('WatchReviewComments', { checkoutId: 'checkout-1' }),
  request('UpsertReviewComment', {
    id: '',
    checkoutId: 'checkout-1',
    path: 'src/main.rs',
    side: 'new',
    line: 12,
    endLine: 14,
    body: 'This loop never ends.',
    diffChecksum: 'sum-1',
    createdAtMs: 0,
    resolved: false,
    outdated: false,
  }),
  request('DeleteReviewComment', { checkoutId: 'checkout-1', id: 'comment-1' }),
  request('ResolveReviewComment', { checkoutId: 'checkout-1', id: 'comment-1', resolved: true }),
  request('SendReviewToThread', { chatId, commentIds: ['comment-1'] }),
  request('PostReviewToChangeRequest', {
    checkoutId: 'checkout-1',
    commentIds: ['comment-1'],
    event: 'request_changes',
  }),
  request('SearchWorkspaceContent', {
    spaceId: 'space-1',
    chatId: null,
    query: 'fn main',
    regex: false,
    caseSensitive: true,
    globs: ['*.rs'],
    maxResults: 200,
  }),
  request('CreateRunGroup', {
    spaceId: 'space-1',
    repoPath: '/home/dev/project',
    base: 'main',
    prompt: 'Fix the build',
    attachments: [],
    variants: [
      { harness: 'claude-code', model: 'claude-sonnet', reasoning: 'high', modelOptions: {} },
      { harness: 'codex', model: null, reasoning: null, modelOptions: {} },
    ],
    checkActionId: 'test',
    approvalMode: 'supervised',
  }),
  request('WatchRunGroups', { spaceId: null }),
  request('GetRunGroupComparison', {
    groupId: 'group-1',
    a: 'chat-1',
    b: 'chat-2',
    path: null,
  }),
  request('RunGroupCheck', { groupId: 'group-1', chatId: null }),
  request('ResolveRunGroup', {
    groupId: 'group-1',
    winnerChatId: 'chat-1',
    action: 'pr',
    merge: null,
    pr: {
      cwd: '/home/dev/.loams-desktop/worktrees/project/brave-otter',
      provider: 'github',
      base: 'main',
      head: 'loams-desktop/brave-otter',
      title: 'Fix the build',
      body: '',
      draft: false,
      linkIssue: null,
    },
    archiveOthers: true,
    removeOtherWorktrees: false,
    force: false,
    expectedChecksum: 'sum-1',
  }),
  request('WatchApprovals', { chatId }),
  request('ResolveApproval', { callId: 'call-1', chatId, decision: 'always' }),
  request('SetThreadApprovalMode', { chatId, mode: 'supervised' }),
  request('GetCheckoutFileDiffText', {
    checkoutId: 'checkout-1',
    cwd: '/home/dev/project',
    path: 'src/main.rs',
    mode: 'turn',
    chatId,
    diffChecksum: 'sum-1',
  }),
  request('ListAgentAccounts', { forceUsage: true }),
  request('ActivateAgentAccount', { harness: 'codex', accountId: 'account-1' }),
  request('ForgetAgentAccount', { harness: 'codex', accountId: 'account-1' }),
  request('StartAgentLogin', { harness: 'opencode', provider: 'anthropic' }),
  request('CompleteAgentLogin', { loginId: 'login-1', code: 'code-1' }),
  request('PollAgentLogin', { loginId: 'login-1' }),
  request('CancelAgentLogin', { loginId: 'login-1' }),
  request('UploadChunk', { uploadId: 'upload-1', data: 'iVBORw0KGgo=', seq: 0 }),
  request('UploadCommit', { uploadId: 'upload-1', fileName: 'shot.png' }),
  request('ReadAttachmentChunk', { path: '/home/dev/.uploads/shot.png', offset: 0 }),
  request('FetchToolBlob', { blobRef: 'chat-1/call-1', offset: 0, len: 262144 }),
  request('WatchHarnessUpdates', {}),
  request('CheckHarnessUpdates', {}),
  request('ApplyHarnessUpdate', { harness: 'codex' }),
  request('CancelHarnessUpdate', { harness: 'codex' }),
  request('DismissHarnessUpdate', { harness: 'codex', version: '0.41.0' }),
  // The factory loop's tools (plan FT1, FT0 Task 4).
  request('CreateRunWorktree', { chatId: 'chat-1', branch: 'factory/run-1/1', base: 'main' }),
  request('GetThreadDiff', { chatId: 'chat-1', baseRef: 'main' }),
  request('PostReviewComment', {
    chatId: 'chat-1',
    path: 'src/app.ts',
    side: 'new',
    line: 3,
    body: 'Check the limit.',
    diffChecksum: 'sum-1',
  }),
  request('OpenChangeRequest', {
    chatId: 'chat-1',
    provider: 'forgejo',
    base: 'main',
    title: 'Rate limit signup',
    body: '',
    draft: false,
  }),
  request('RequestApproval', {
    chatId: 'chat-1',
    kind: 'merge',
    summary: 'Merge #7',
    payload: { repo: 'dev/app', pr: 7, head_sha: 'abcdef0' },
  }),
  request('GetApproval', { chatId: 'chat-1', approvalId: 'fa-1' }),
  request('MergeChangeRequest', {
    chatId: 'chat-1',
    approvalId: 'fa-1',
    provider: 'forgejo',
    number: 7,
    headSha: 'abcdef0',
  }),
  request('GetChecks', { chatId: 'chat-1', provider: 'github', sha: 'abcdef0' }),
  request('GetCheckLog', { chatId: 'chat-1', provider: 'github', logId: '901', lines: 200 }),
  request('DeployLiveApp', {
    chatId: 'chat-1',
    approvalId: 'fa-2',
    app: 'notes',
    commit: '0123456789abcdef0123456789abcdef01234567',
  }),
  request('CloseChangeRequest', { chatId: 'chat-1', provider: 'forgejo', number: 7 }),
];

const file = join(import.meta.dirname, 'requests.json');

describe('requests.json', () => {
  it('has a request for every method', () => {
    const covered = new Set(REQUESTS.map((r) => r.method));
    expect(METHODS.filter((m) => !covered.has(m.name)).map((m) => m.name)).toEqual([]);
  });

  it('is what the typed builders write', () => {
    const text = `${JSON.stringify(REQUESTS, null, 2)}\n`;
    if (process.env.UPDATE_REQUESTS === '1') {
      writeFileSync(file, text);
    }
    expect(readFileSync(file, 'utf8')).toBe(text);
  });
});

describe('Request', () => {
  it('pairs each method with its own params', () => {
    // @ts-expect-error `Hello` cannot carry another method's params.
    const mismatched: Request = { method: 'Hello', params: { chatId: 'chat-1' } };
    const paired: Request<'WatchQueue'> = request('WatchQueue', { chatId: 'chat-1' });
    expect([mismatched.method, paired.method]).toEqual(['Hello', 'WatchQueue']);
  });
});
