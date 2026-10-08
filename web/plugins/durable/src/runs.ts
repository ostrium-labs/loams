// Runs: promises grouped into trees by lineage. The pinned resonate SDK
// (e360669, impl/sdk/ts/src/resonate.ts, context.ts) tags every promise of a
// run with `resonate:origin` (the root promise's id), `resonate:branch` and
// `resonate:parent` (the id of the promise that spawned it). A root has
// origin == parent == its own id. There is no `resonate:root` tag.

import type { PromiseRecord } from './envelope.js';

export const TAG_ORIGIN = 'resonate:origin';
export const TAG_PARENT = 'resonate:parent';

export interface RunNode {
  promise: PromiseRecord;
  children: RunNode[];
}

const byCreated = (a: RunNode, b: RunNode) =>
  a.promise.createdAt - b.promise.createdAt || a.promise.id.localeCompare(b.promise.id);

/**
 * The run trees in `promises`. A promise without a `resonate:origin` tag is not
 * part of a run and is left out. A child whose parent is missing from the list
 * (a page boundary) hangs off its run's root when that is present, else it
 * becomes a root itself so nothing is lost.
 */
export function buildRuns(promises: PromiseRecord[]): RunNode[] {
  const nodes = new Map<string, RunNode>();
  for (const p of promises) if (p.tags[TAG_ORIGIN]) nodes.set(p.id, { promise: p, children: [] });
  const roots: RunNode[] = [];
  for (const node of nodes.values()) {
    const { id, tags } = node.promise;
    const parentId = tags[TAG_PARENT];
    const parent =
      parentId && parentId !== id
        ? (nodes.get(parentId) ?? nodes.get(tags[TAG_ORIGIN] ?? ''))
        : undefined;
    if (parent && parent !== node) parent.children.push(node);
    else roots.push(node);
  }
  for (const n of nodes.values()) n.children.sort(byCreated);
  return roots.sort(byCreated).reverse();
}

export function countNodes(n: RunNode): number {
  return 1 + n.children.reduce((s, c) => s + countNodes(c), 0);
}
