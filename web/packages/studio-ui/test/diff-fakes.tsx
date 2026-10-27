// A thread's diffs and review comments in memory for the diff panel's tests (SD1 Task 20).

import { vi } from 'vite-plus/test';
import type {
  ChangeRequestInfo,
  CheckoutDiffView,
  DiffSelection,
  DiffSource,
  ReviewComment,
  ReviewCommentDraft,
} from '../src/diff/source';
import { StaleDiffError } from '../src/diff/source';

export const WORKING_PATCH = `diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,4 +1,4 @@
 export const a = 1;
-export const b = "<img src=x onerror=alert(1)>";
+export const b = "<script>window.__pwned = 1</script>";
 export const c = 3;
 export const d = 4;
diff --git a/README.md b/README.md
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Title
+<a href="javascript:alert(1)">link</a>
`;

export const BRANCH_PATCH = `diff --git a/src/branch.ts b/src/branch.ts
index 1111111..2222222 100644
--- a/src/branch.ts
+++ b/src/branch.ts
@@ -1,2 +1,2 @@
-export const branch = 1;
+export const branch = 2;
 export const keep = true;
`;

export const COMMIT_PATCH = `diff --git a/src/commit.ts b/src/commit.ts
index 1111111..2222222 100644
--- a/src/commit.ts
+++ b/src/commit.ts
@@ -1 +1 @@
-export const commit = 1;
+export const commit = 2;
`;

export const TURN_PATCH = `diff --git a/src/turn.ts b/src/turn.ts
index 1111111..2222222 100644
--- a/src/turn.ts
+++ b/src/turn.ts
@@ -1 +1 @@
-export const turn = 1;
+export const turn = 2;
`;

export function checkoutDiff(patch: string, checksum: string): CheckoutDiffView {
  return {
    checkoutId: 'co-1',
    cwd: '/repo',
    patch,
    files: [
      { path: 'src/app.ts', status: 'modified', additions: 1, deletions: 1, binary: false },
      { path: 'README.md', status: 'added', additions: 2, deletions: 0, binary: false },
    ],
    additions: 3,
    deletions: 1,
    truncated: false,
    checksum,
    updatedAtMs: 1,
  };
}

export function comment(over: Partial<ReviewComment> = {}): ReviewComment {
  return {
    id: 'c1',
    checkoutId: 'co-1',
    path: 'src/app.ts',
    side: 'new',
    line: 2,
    body: 'Why a <script> here?',
    diffChecksum: 'sum-1',
    createdAtMs: 1,
    resolved: false,
    outdated: false,
    ...over,
  };
}

export interface FakeDiff {
  source: DiffSource;
  getDiff: ReturnType<typeof vi.fn>;
  upsert: ReviewCommentDraft[];
  sendReview: ReturnType<typeof vi.fn>;
  postReview: ReturnType<typeof vi.fn>;
  discardAll: ReturnType<typeof vi.fn>;
  discardPaths: ReturnType<typeof vi.fn>;
  setComments(comments: ReviewComment[]): void;
  setChangeRequest(changeRequest: ChangeRequestInfo | null): void;
  /** Sends the working tree's watch a snapshot. */
  emitWorking(diff: CheckoutDiffView): void;
}

export function fakeDiff(
  opts: {
    comments?: ReviewComment[];
    changeRequest?: ChangeRequestInfo | null;
    checksum?: string;
    stale?: boolean;
  } = {},
): FakeDiff {
  const checksum = opts.checksum ?? 'sum-1';
  let comments = opts.comments ?? [];
  let changeRequest = opts.changeRequest ?? null;
  const commentListeners = new Set<(c: readonly ReviewComment[]) => void>();
  const crListeners = new Set<(c: ChangeRequestInfo | null) => void>();
  const workingListeners = new Set<(d: CheckoutDiffView) => void>();
  const upsert: ReviewCommentDraft[] = [];
  const getDiff = vi.fn(async (selection: DiffSelection) => {
    switch (selection.kind) {
      case 'branch':
        return checkoutDiff(BRANCH_PATCH, `branch-${selection.baseRef}`);
      case 'commit':
        return checkoutDiff(COMMIT_PATCH, `commit-${selection.commitSha}`);
      case 'turn':
        return checkoutDiff(TURN_PATCH, 'turn');
      default:
        return checkoutDiff(WORKING_PATCH, checksum);
    }
  });
  const sendReview = vi.fn(async () => {});
  const postReview = vi.fn(async () => 'https://forge.example/pr/7#review');
  const guard = async () => {
    if (opts.stale) throw new StaleDiffError();
  };
  const discardAll = vi.fn(guard);
  const discardPaths = vi.fn(guard);
  const source: DiffSource = {
    key: 'fake:thread-1',
    watchWorkingDiff(listener) {
      workingListeners.add(listener);
      listener(checkoutDiff(WORKING_PATCH, checksum));
      return () => workingListeners.delete(listener);
    },
    getDiff,
    async fileText() {
      return { oldText: '', newText: '', binary: false, truncated: false, stale: false };
    },
    async listBranches() {
      return ['feature', 'main'];
    },
    async listCommits() {
      return [
        { sha: 'abcdef0123456789', subject: 'Latest' },
        { sha: '0123456789abcdef', subject: 'Older' },
      ];
    },
    watchComments(_checkoutId, listener) {
      commentListeners.add(listener);
      listener(comments);
      return () => commentListeners.delete(listener);
    },
    async upsertComment(draft) {
      upsert.push(draft);
      return draft.id;
    },
    async deleteComment() {},
    async resolveComment() {},
    sendReviewToThread: sendReview,
    postReviewToChangeRequest: postReview,
    watchChangeRequest(listener) {
      crListeners.add(listener);
      listener(changeRequest);
      return () => crListeners.delete(listener);
    },
    discardAll,
    discardPaths,
  };
  return {
    source,
    getDiff,
    upsert,
    sendReview,
    postReview,
    discardAll,
    discardPaths,
    setComments(next) {
      comments = next;
      for (const listener of commentListeners) listener(next);
    },
    setChangeRequest(next) {
      changeRequest = next;
      for (const listener of crListeners) listener(next);
    },
    emitWorking(next) {
      for (const listener of workingListeners) listener(next);
    },
  };
}
