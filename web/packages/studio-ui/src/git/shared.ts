// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// packages/shared/src/git.ts at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// the ref-name helpers the branch toolbar and git actions use, in plain TypeScript (no effect/Array or Result); the temporary worktree branch is loams-agentd's `loams-desktop/<name>`; remote-URL and status-stream helpers dropped (loams-agentd answers them); a forge repository parser for pull request URLs added; the fragment's edge trims made linear-time (CodeQL js/polynomial-redos). Loams' changes are Apache-2.0.

// Git ref-name helpers for Studio's branch toolbar and git actions (SD1 Task 23).

import { trimEndWhere, trimWhere } from "../lib/trimWhere";

/** loams-agentd's `CreateWorktree` puts a new worktree on `loams-desktop/<adjective>-<noun>`. */
export const WORKTREE_BRANCH_PREFIX = "loams-desktop";
const TEMP_WORKTREE_BRANCH_PATTERN = new RegExp(`^${WORKTREE_BRANCH_PREFIX}/[a-z]+-[a-z]+$`);

/**
 * Sanitize an arbitrary string into a valid, lowercase git refName fragment.
 * Strips quotes, collapses separators, limits to 64 chars.
 */
export function sanitizeBranchFragment(raw: string): string {
  // The edge runs are trimmed in a loop: `/^[…]+|[…]+$/g` is quadratic on a long run
  // of those characters that does not reach the end (CodeQL js/polynomial-redos).
  const normalized = trimWhere(raw.trim().toLowerCase().replace(/['"`]/g, ""), isEdgeOrSpace);

  const branchFragment = trimEndWhere(
    trimWhere(
      normalized
        .replace(/[^a-z0-9/_-]+/g, "-")
        .replace(/\/+/g, "/")
        .replace(/-+/g, "-"),
      isEdge,
    ).slice(0, 64),
    isEdge,
  );

  return branchFragment.length > 0 ? branchFragment : "update";
}

/** The characters a ref-name fragment may not start or end with. */
function isEdge(char: string): boolean {
  return char === "." || char === "/" || char === "_" || char === "-";
}

function isEdgeOrSpace(char: string): boolean {
  return isEdge(char) || /\s/.test(char);
}

/**
 * Sanitize a generated branch name, preserving any slash-separated namespace.
 * Unprefixed fragments use the `feature/` namespace.
 */
export function sanitizeFeatureBranchName(raw: string): string {
  const sanitized = sanitizeBranchFragment(raw);
  return sanitized.includes("/") ? sanitized : `feature/${sanitized}`;
}

const AUTO_FEATURE_BRANCH_FALLBACK = "feature/update";

/**
 * Resolve a unique generated refName, suffixing only the path component
 * blocked by an existing ref or, for the final component, its descendants.
 */
export function resolveAutoFeatureBranchName(
  existingBranchNames: readonly string[],
  preferredBranch?: string,
): string {
  const preferred = preferredBranch?.trim();
  const resolvedBase = sanitizeFeatureBranchName(
    preferred && preferred.length > 0 ? preferred : AUTO_FEATURE_BRANCH_FALLBACK,
  );
  const existingNames = existingBranchNames.map((refName) => refName.toLowerCase());
  const existingRefs = new Set(existingNames);
  const parts = resolvedBase.split("/");
  let resolved = "";

  for (const [index, part] of parts.entries()) {
    const base = resolved ? `${resolved}/${part}` : part;
    let candidate = base;
    let suffix = 2;
    while (
      existingRefs.has(candidate) ||
      (index === parts.length - 1 &&
        existingNames.some((refName) => refName.startsWith(`${candidate}/`)))
    ) {
      candidate = `${base}-${suffix}`;
      suffix += 1;
    }
    resolved = candidate;
  }

  return resolved;
}

/**
 * Strip the remote prefix from a remote ref such as `origin/feature/demo`.
 */
export function deriveLocalBranchNameFromRemoteRef(branchName: string): string {
  const firstSeparatorIndex = branchName.indexOf("/");
  if (firstSeparatorIndex <= 0 || firstSeparatorIndex === branchName.length - 1) {
    return branchName;
  }
  return branchName.slice(firstSeparatorIndex + 1);
}

export function isTemporaryWorktreeBranch(refName: string): boolean {
  return TEMP_WORKTREE_BRANCH_PATTERN.test(refName.trim().toLowerCase());
}

// Git rejects ASCII space and the ASCII control characters (tab, newline and
// friends) in ref names, so a typed name like "new branch" can only fail.
// Replacing runs of those with a dash makes the name usable without
// reimplementing check-ref-format: names invalid for other reasons still
// surface the git error. Only the whitespace git actually rejects is replaced.
export function sanitizeNewRefName(rawName: string): string {
  return rawName.trim().replace(/[ \t\n\r\f\v]+/g, "-");
}

/**
 * `owner/name` from a forge's pull request or repository URL
 * (`https://git.example.com/acme/site/pulls/12`, `https://github.com/acme/site.git`),
 * or null. http(s) only.
 */
export function forgeRepositoryOfUrl(url: string | null | undefined): string | null {
  if (!url) return null;
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return null;
  const [owner, name] = parsed.pathname.split("/").filter(Boolean);
  const repo = name?.replace(/\.git$/, "");
  const part = /^[A-Za-z0-9._-]+$/;
  return owner && repo && part.test(owner) && part.test(repo) ? `${owner}/${repo}` : null;
}
