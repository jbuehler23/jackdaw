// Pure force-directed layout for the relationship graph. No DOM/canvas access;
// callers own rendering, dragging (pin a node by setting its x/y directly and
// bumping alpha) and navigation (this module never clamps positions to a canvas).

export interface GraphNode {
  id: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
}

export interface GraphEdge {
  a: number;
  b: number;
  kind: 'child';
}

export interface LayoutState {
  nodes: Map<number, GraphNode>;
  alpha: number;
}

const ALPHA_FLOOR = 0.02;
const ALPHA_DECAY = 0.985;
const DAMPING = 0.86;
const CENTER_GRAVITY = 0.0009;
const SPRING_K = 0.012;
const CHILD_REST_LENGTH = 105;
const REPULSE_CELL = 70;
const REPULSE_NUMERIC_LIMIT = 120;
const GOLDEN_ANGLE = 2.399963;
const WARM_START_STEPS = 180;

/** Reconciles the node map against the current entity ids: seeds new nodes near
 * their parent (or scattered around the center for roots), prunes removed ids,
 * and bumps alpha so the layout re-settles when membership changes. */
export function syncNodes(
  state: LayoutState,
  ids: number[],
  parents: Map<number, number | null>,
  w: number,
  h: number,
): void {
  let changed = 0;
  let i = 0;
  for (const id of ids) {
    if (!state.nodes.has(id)) {
      const parentId = parents.get(id);
      const parent = parentId != null ? state.nodes.get(parentId) : undefined;
      const ang = i * GOLDEN_ANGLE;
      const r0 = 60 + 18 * Math.sqrt(i);
      const x = parent ? parent.x + (Math.random() - 0.5) * 80 : w / 2 + Math.cos(ang) * r0;
      const y = parent ? parent.y + (Math.random() - 0.5) * 80 : h / 2 + Math.sin(ang) * r0;
      state.nodes.set(id, { id, x, y, vx: 0, vy: 0 });
      changed++;
    }
    i++;
  }
  const idSet = new Set(ids);
  for (const nodeId of [...state.nodes.keys()]) {
    if (!idSet.has(nodeId)) {
      state.nodes.delete(nodeId);
      changed++;
    }
  }
  if (changed > 0) state.alpha = Math.max(state.alpha, 0.7);
}

/** Pushes two nodes apart with force min(3200/d2, 3); nudges randomly apart
 * when they nearly coincide. `half` halves the force and skips the reaction
 * on `nb` (used for the spatial-hash pass, where each pair is visited twice). */
export function applyRepulse(na: GraphNode, nb: GraphNode, half = false): void {
  let dx = na.x - nb.x;
  let dy = na.y - nb.y;
  let d2 = dx * dx + dy * dy;
  if (d2 < 1) {
    dx = Math.random() - 0.5;
    dy = Math.random() - 0.5;
    d2 = 1;
  }
  const f = Math.min(3200 / d2, 3) * (half ? 0.5 : 1);
  const d = Math.sqrt(d2);
  na.vx += (dx / d) * f;
  na.vy += (dy / d) * f;
  if (!half) {
    nb.vx -= (dx / d) * f;
    nb.vy -= (dy / d) * f;
  }
}

/** Applies pairwise repulsion across the visible nodes: O(n^2) for small
 * graphs, a 70px spatial hash (with half-strength neighbor-cell pushes) above
 * the threshold where the quadratic pass gets too expensive. */
export function relRepulsion(entries: GraphNode[]): void {
  const n = entries.length;
  if (n <= REPULSE_NUMERIC_LIMIT) {
    for (let a = 0; a < n; a++) {
      for (let b = a + 1; b < n; b++) applyRepulse(entries[a], entries[b]);
    }
    return;
  }
  const grid = new Map<string, GraphNode[]>();
  for (const node of entries) {
    const key = `${Math.floor(node.x / REPULSE_CELL)},${Math.floor(node.y / REPULSE_CELL)}`;
    const cell = grid.get(key);
    if (cell) cell.push(node);
    else grid.set(key, [node]);
  }
  for (const node of entries) {
    const cx = Math.floor(node.x / REPULSE_CELL);
    const cy = Math.floor(node.y / REPULSE_CELL);
    for (let gx = cx - 1; gx <= cx + 1; gx++) {
      for (let gy = cy - 1; gy <= cy + 1; gy++) {
        const cell = grid.get(`${gx},${gy}`);
        if (!cell) continue;
        for (const other of cell) {
          if (other !== node) applyRepulse(node, other, true);
        }
      }
    }
  }
}

/** Advances the layout one tick: repulsion, spring edges, center gravity,
 * damping, alpha-scaled position integration, then cools alpha. No-ops once
 * alpha has decayed below the floor. */
export function step(state: LayoutState, edges: GraphEdge[], visible: Set<number>, w: number, h: number): void {
  if (state.alpha <= ALPHA_FLOOR) return;

  const entries: GraphNode[] = [];
  for (const [id, node] of state.nodes) {
    if (visible.has(id)) entries.push(node);
  }

  relRepulsion(entries);

  for (const edge of edges) {
    if (!visible.has(edge.a) || !visible.has(edge.b)) continue;
    const a = state.nodes.get(edge.a);
    const b = state.nodes.get(edge.b);
    if (!a || !b) continue;
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const d = Math.hypot(dx, dy) || 1;
    const rest = CHILD_REST_LENGTH;
    const f = (d - rest) * SPRING_K;
    a.vx += (dx / d) * f;
    a.vy += (dy / d) * f;
    b.vx -= (dx / d) * f;
    b.vy -= (dy / d) * f;
  }

  const k = state.alpha;
  for (const node of entries) {
    node.vx += (w / 2 - node.x) * CENTER_GRAVITY;
    node.vy += (h / 2 - node.y) * CENTER_GRAVITY;
    node.vx *= DAMPING;
    node.vy *= DAMPING;
    node.x += node.vx * k;
    node.y += node.vy * k;
  }

  state.alpha *= ALPHA_DECAY;
}

/** Resets alpha to 1 and runs 180 synchronous steps so the layout settles
 * before the first paint, avoiding a drift-to-center animation. */
export function warmStart(state: LayoutState, edges: GraphEdge[], visible: Set<number>, w: number, h: number): void {
  state.alpha = 1;
  for (let i = 0; i < WARM_START_STEPS; i++) step(state, edges, visible, w, h);
}

/** Computes a view transform that fits every node's bounding box into the
 * canvas with padding, clamped to a sane zoom range. */
export function fitView(nodes: Iterable<GraphNode>, w: number, h: number): { x: number; y: number; k: number } {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  let any = false;
  for (const node of nodes) {
    any = true;
    x0 = Math.min(x0, node.x);
    y0 = Math.min(y0, node.y);
    x1 = Math.max(x1, node.x);
    y1 = Math.max(y1, node.y);
  }
  if (!any) return { x: 0, y: 0, k: 1 };

  const bw = Math.max(60, x1 - x0);
  const bh = Math.max(60, y1 - y0);
  const k = Math.max(0.15, Math.min(2.5, Math.min(w / bw, h / bh) * 0.82));
  return {
    k,
    x: w / 2 - ((x0 + x1) / 2) * k,
    y: h / 2 - ((y0 + y1) / 2) * k,
  };
}
