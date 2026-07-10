import { describe, expect, it } from 'vitest';
import { fitView, step, syncNodes, warmStart, type GraphEdge, type LayoutState } from './graph-layout';

function emptyState(): LayoutState {
  return { nodes: new Map(), alpha: 1 };
}

function chainGraph(n: number): { state: LayoutState; edges: GraphEdge[]; visible: Set<number> } {
  const state = emptyState();
  const parents = new Map<number, number | null>();
  const ids: number[] = [];
  for (let i = 0; i < n; i++) {
    ids.push(i);
    parents.set(i, i === 0 ? null : i - 1);
  }
  syncNodes(state, ids, parents, 800, 600);
  const edges: GraphEdge[] = [];
  for (let i = 1; i < n; i++) edges.push({ a: i, b: i - 1, kind: 'child' });
  return { state, edges, visible: new Set(ids) };
}

// One root with many direct children, like a typical scene hierarchy (a
// single parent with several sibling entities), rather than a long chain.
function starGraph(n: number): { state: LayoutState; edges: GraphEdge[]; visible: Set<number> } {
  const state = emptyState();
  const parents = new Map<number, number | null>();
  const ids: number[] = [];
  for (let i = 0; i < n; i++) {
    ids.push(i);
    parents.set(i, i === 0 ? null : 0);
  }
  syncNodes(state, ids, parents, 800, 600);
  const edges: GraphEdge[] = [];
  for (let i = 1; i < n; i++) edges.push({ a: i, b: 0, kind: 'child' });
  return { state, edges, visible: new Set(ids) };
}

describe('warmStart', () => {
  it('settles: average node speed is low after warm start', () => {
    const { state, edges, visible } = starGraph(20);
    warmStart(state, edges, visible, 800, 600);
    let total = 0;
    let count = 0;
    for (const node of state.nodes.values()) {
      total += Math.hypot(node.vx, node.vy);
      count++;
    }
    expect(total / count).toBeLessThan(0.5);
  });
});

describe('structure', () => {
  it('places a child nearer its parent than an unrelated root', () => {
    const state = emptyState();
    const parents = new Map<number, number | null>([
      [0, null],
      [1, 0],
      [2, null],
    ]);
    syncNodes(state, [0, 1, 2], parents, 800, 600);
    const edges: GraphEdge[] = [{ a: 1, b: 0, kind: 'child' }];
    const visible = new Set([0, 1, 2]);
    warmStart(state, edges, visible, 800, 600);

    const root = state.nodes.get(0)!;
    const child = state.nodes.get(1)!;
    const unrelated = state.nodes.get(2)!;

    const distChildToRoot = Math.hypot(child.x - root.x, child.y - root.y);
    const distUnrelatedToRoot = Math.hypot(unrelated.x - root.x, unrelated.y - root.y);
    expect(distChildToRoot).toBeLessThan(distUnrelatedToRoot);
  });
});

describe('performance', () => {
  it('runs 60 steps over 150 nodes / 149 edges under 400ms', () => {
    const { state, edges, visible } = chainGraph(150);
    state.alpha = 1;
    const start = performance.now();
    for (let i = 0; i < 60; i++) step(state, edges, visible, 1200, 900);
    const elapsed = performance.now() - start;
    expect(elapsed).toBeLessThan(400);
  });
});

describe('fitView', () => {
  it('bounds every node within the canvas with margin and centers the bbox', () => {
    const nodes = [
      { id: 0, x: -100, y: -50, vx: 0, vy: 0 },
      { id: 1, x: 100, y: 50, vx: 0, vy: 0 },
      { id: 2, x: 0, y: 0, vx: 0, vy: 0 },
    ];
    const w = 800;
    const h = 600;
    const view = fitView(nodes, w, h);

    expect(view.k).toBeGreaterThanOrEqual(0.15);
    expect(view.k).toBeLessThanOrEqual(2.5);

    const margin = 40;
    for (const node of nodes) {
      const sx = node.x * view.k + view.x;
      const sy = node.y * view.k + view.y;
      expect(sx).toBeGreaterThanOrEqual(-margin);
      expect(sx).toBeLessThanOrEqual(w + margin);
      expect(sy).toBeGreaterThanOrEqual(-margin);
      expect(sy).toBeLessThanOrEqual(h + margin);
    }

    // bbox center (0, 0) should map near the canvas center
    const centerX = 0 * view.k + view.x;
    const centerY = 0 * view.k + view.y;
    expect(centerX).toBeCloseTo(w / 2, 0);
    expect(centerY).toBeCloseTo(h / 2, 0);
  });
});

describe('alpha decay', () => {
  it('decays below the floor and step() becomes a no-op', () => {
    const { state, edges, visible } = chainGraph(5);
    state.alpha = 1;
    while (state.alpha > 0.02) step(state, edges, visible, 800, 600);
    expect(state.alpha).toBeLessThanOrEqual(0.02);

    const snapshot = new Map([...state.nodes].map(([id, n]) => [id, { ...n }]));
    step(state, edges, visible, 800, 600);
    for (const [id, node] of state.nodes) {
      const before = snapshot.get(id)!;
      expect(node.x).toBe(before.x);
      expect(node.y).toBe(before.y);
      expect(node.vx).toBe(before.vx);
      expect(node.vy).toBe(before.vy);
    }
    expect(state.alpha).toBeLessThanOrEqual(0.02);
  });
});

describe('syncNodes', () => {
  it('prunes removed ids and seeds a new child near its parent', () => {
    const state = emptyState();
    const parents = new Map<number, number | null>([
      [0, null],
      [1, 0],
    ]);
    syncNodes(state, [0, 1], parents, 800, 600);
    state.nodes.get(0)!.x = 400;
    state.nodes.get(0)!.y = 300;

    // entity 1 is removed, entity 2 (child of 0) is added
    const parents2 = new Map<number, number | null>([
      [0, null],
      [2, 0],
    ]);
    syncNodes(state, [0, 2], parents2, 800, 600);

    expect(state.nodes.has(1)).toBe(false);
    expect(state.nodes.has(2)).toBe(true);

    const root = state.nodes.get(0)!;
    const child = state.nodes.get(2)!;
    const dist = Math.hypot(child.x - root.x, child.y - root.y);
    expect(dist).toBeLessThan(200);
    expect(state.alpha).toBeGreaterThanOrEqual(0.7);
  });
});
