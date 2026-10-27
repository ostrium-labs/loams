// The in-repo layered layout and the keyboard navigation geometry.

import { describe, expect, it } from 'vite-plus/test';
import { layeredLayout, NODE_HEIGHT, NODE_WIDTH, nextFreePosition } from '../src/layout.js';
import { nearestInDirection, readingOrder } from '../src/navigate.js';

const column = (x: number) => Math.round(x / (NODE_WIDTH + 96));

describe('layeredLayout', () => {
  it('puts each node one column right of its furthest predecessor', () => {
    const pos = layeredLayout(
      ['a', 'b', 'c', 'd'],
      [
        { source: 'a', target: 'b' },
        { source: 'b', target: 'c' },
        { source: 'a', target: 'c' },
        { source: 'c', target: 'd' },
      ],
    );
    expect([pos.a, pos.b, pos.c, pos.d].map((p) => column(p?.x ?? -1))).toEqual([0, 1, 2, 3]);
  });

  it('places a node with no edges in the first column', () => {
    const pos = layeredLayout(['lonely', 'a', 'b'], [{ source: 'a', target: 'b' }]);
    expect(column(pos.lonely?.x ?? -1)).toBe(0);
    expect(column(pos.b?.x ?? -1)).toBe(1);
  });

  it('survives cycles, self-loops and edges to unknown ids', () => {
    const pos = layeredLayout(
      ['a', 'b', 'c'],
      [
        { source: 'a', target: 'b' },
        { source: 'b', target: 'c' },
        { source: 'c', target: 'a' },
        { source: 'a', target: 'a' },
        { source: 'a', target: 'ghost' },
      ],
    );
    expect(Object.keys(pos).sort()).toEqual(['a', 'b', 'c']);
    expect([pos.a, pos.b, pos.c].map((p) => column(p?.x ?? -1))).toEqual([0, 1, 2]);
  });

  it('never overlaps two nodes in one column', () => {
    const ids = ['src', 'x1', 'x2', 'x3', 'x4'];
    const pos = layeredLayout(
      ids,
      ids.slice(1).map((t) => ({ source: 'src', target: t })),
    );
    const ys = ids
      .slice(1)
      .map((id) => pos[id]?.y ?? 0)
      .sort((a, b) => a - b);
    for (let i = 1; i < ys.length; i++) {
      expect((ys[i] as number) - (ys[i - 1] as number)).toBeGreaterThanOrEqual(NODE_HEIGHT);
    }
  });

  it('orders a layer to follow its parents (fewer crossings)', () => {
    // p1 -> c1, p2 -> c2, given in the crossed order c2, c1.
    const pos = layeredLayout(
      ['p1', 'p2', 'c2', 'c1'],
      [
        { source: 'p1', target: 'c1' },
        { source: 'p2', target: 'c2' },
      ],
    );
    expect((pos.p1?.y ?? 0) < (pos.p2?.y ?? 0)).toBe(true);
    expect((pos.c1?.y ?? 0) < (pos.c2?.y ?? 0)).toBe(true);
  });

  it('is deterministic', () => {
    const edges = [
      { source: 'a', target: 'b' },
      { source: 'a', target: 'c' },
    ];
    expect(layeredLayout(['a', 'b', 'c'], edges)).toEqual(layeredLayout(['a', 'b', 'c'], edges));
  });

  it('returns nothing for no nodes', () => {
    expect(layeredLayout([], [])).toEqual({});
  });
});

describe('nextFreePosition', () => {
  it('starts at the origin and then goes below the lowest node', () => {
    expect(nextFreePosition([])).toEqual({ x: 0, y: 0 });
    expect(
      nextFreePosition([
        { x: 50, y: 0 },
        { x: 400, y: 300 },
      ]),
    ).toEqual({ x: 50, y: 300 + NODE_HEIGHT + 40 });
  });
});

describe('nearestInDirection', () => {
  const nodes = [
    { id: 'centre', position: { x: 0, y: 0 } },
    { id: 'right-far-inline', position: { x: 400, y: 0 } },
    { id: 'right-near-offset', position: { x: 200, y: 300 } },
    { id: 'below', position: { x: 0, y: 200 } },
    { id: 'left', position: { x: -300, y: 10 } },
  ];

  it('prefers a node in line over a nearer one off to the side', () => {
    expect(nearestInDirection(nodes, 'centre', 'right')).toBe('right-far-inline');
  });

  it('finds each direction and nothing where nothing lies', () => {
    expect(nearestInDirection(nodes, 'centre', 'down')).toBe('below');
    expect(nearestInDirection(nodes, 'centre', 'left')).toBe('left');
    expect(nearestInDirection(nodes, 'centre', 'up')).toBeUndefined();
    expect(nearestInDirection(nodes, 'missing', 'up')).toBeUndefined();
  });

  it('reads nodes top to bottom, then left to right', () => {
    expect(readingOrder(nodes)).toEqual([
      'centre',
      'right-far-inline',
      'left',
      'below',
      'right-near-offset',
    ]);
  });
});
