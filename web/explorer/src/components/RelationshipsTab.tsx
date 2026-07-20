// RelationshipsTab.tsx: canvas force graph of the entity hierarchy. Only
// ChildOf edges are drawn: custom relationship components aren't discoverable
// over stock BRP metadata, so the legend says so rather than pretending
// otherwise.
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { Braces, Boxes, CircleDot, RotateCw, Search } from 'lucide-preact';
import { Icon } from './Icon';
import {
  fitView,
  step,
  syncNodes,
  warmStart,
  type GraphEdge,
  type GraphNode,
  type LayoutState,
} from '../lib/graph-layout';
import { treePoll } from '../lib/treeData';
import { classifyKind, parentEntity, type EntityKind, CHILD_OF, NAME } from '../lib/tree';
import type { QueryRow } from '../lib/brp';
import { jackdaw } from '../lib/brp';
import { capabilities } from '../lib/connection';
import { page, selectedEntity } from '../lib/state';
import { entityLabel } from '../lib/format';
import { toast } from '../lib/toasts';

interface GraphData {
  ids: number[];
  parents: Map<number, number | null>;
  edges: GraphEdge[];
  kinds: Map<number, EntityKind>;
  names: Map<number, string | null>;
}

interface Point {
  x: number;
  y: number;
}

interface ViewTransform {
  x: number;
  y: number;
  k: number;
}

interface Size {
  width: number;
  height: number;
  dpr: number;
}

type DragState = { mode: 'pan'; last: Point; moved: boolean } | { mode: 'node'; id: number; moved: boolean };

const KIND_COLOR: Record<EntityKind, string> = {
  camera: '#4A9BDB',
  light: '#FFE100',
  mesh: '#B58A4B',
  prefab: '#F2B34D',
  entity: '#42B983',
};

const MANY_THRESHOLD = 60;
const PICK_RADIUS = 12;
const ZOOM_MIN = 0.15;
const ZOOM_MAX = 3.5;

export function shouldWarm(needsWarm: boolean, idCount: number): boolean {
  return needsWarm && idCount > 0;
}

/** Whether the graph body (seeding, warm start, fitView, stepping, drawing)
 * may run this frame. The canvas measures 0x0 on the first RAF ticks (and
 * always in jsdom, which is exactly the bug this guards against): running the
 * body then would scatter every node around (0, 0) and consume the warm flag
 * before there's a real size to fit against. */
export function canRunFrame(width: number, height: number): boolean {
  return width > 0 && height > 0;
}

/** Assembles world.query rows into graph data: every entity id maps to a
 * parent (null for roots), a ChildOf edge per non-root, and per-id kind/name
 * lookups. Pure so it can be unit-tested without mounting the canvas. */
export function buildGraphData(rows: QueryRow[]): GraphData {
  const ids: number[] = [];
  const parents = new Map<number, number | null>();
  const edges: GraphEdge[] = [];
  const kinds = new Map<number, EntityKind>();
  const names = new Map<number, string | null>();

  const idSet = new Set(rows.map((r) => r.entity));
  for (const row of rows) {
    ids.push(row.entity);
    kinds.set(row.entity, classifyKind(row.has));
    const name = row.components[NAME];
    names.set(row.entity, typeof name === 'string' ? name : null);
    const parent = parentEntity(row.components[CHILD_OF]);
    const resolvedParent = parent !== null && idSet.has(parent) ? parent : null;
    parents.set(row.entity, resolvedParent);
    if (resolvedParent !== null) edges.push({ a: row.entity, b: resolvedParent, kind: 'child' });
  }

  return { ids, parents, edges, kinds, names };
}

/** Filters visible ids by name substring (keeping direct neighbors of any
 * match) and, when focus mode is on, to the selection plus its 2-hop
 * neighborhood. Pure so filter/focus logic is testable without a canvas. */
export function computeVisible(
  graph: GraphData,
  filter: string,
  focus: boolean,
  selected: number | null,
): Set<number> {
  let ids = new Set(graph.ids);
  const q = filter.trim().toLowerCase();
  if (q) {
    const match = new Set(graph.ids.filter((id) => (graph.names.get(id) ?? entityLabel(id)).toLowerCase().includes(q)));
    const grown = new Set(match);
    for (const edge of graph.edges) {
      if (match.has(edge.a)) grown.add(edge.b);
      if (match.has(edge.b)) grown.add(edge.a);
    }
    ids = grown;
  }
  if (focus && selected !== null && ids.has(selected)) {
    const keep = new Set([selected]);
    for (let hop = 0; hop < 2; hop++) {
      for (const edge of graph.edges) {
        if (keep.has(edge.a)) keep.add(edge.b);
        if (keep.has(edge.b)) keep.add(edge.a);
      }
    }
    ids = new Set([...ids].filter((id) => keep.has(id)));
  }
  return ids;
}

/** Walks the parent chain to build a `/root/.../name` path, matching the
 * Inspector's path convention. */
export function entityPath(id: number, parents: Map<number, number | null>, names: Map<number, string | null>): string {
  const chain: string[] = [];
  let cur: number | undefined = id;
  while (cur !== undefined) {
    chain.unshift(names.get(cur) ?? entityLabel(cur));
    const parent = parents.get(cur);
    cur = parent != null ? parent : undefined;
  }
  return `/${chain.join('/')}`;
}

/** Finds the node under a screen point, in graph coordinates via the view
 * transform, within a fixed screen-space pick radius. Exported so double-click
 * selection (impractical to simulate through jsdom canvas pointer events) can
 * be covered directly. */
export function pickNode(m: Point, view: ViewTransform, nodes: Map<number, GraphNode>): { id: number; node: GraphNode } | null {
  const g = { x: (m.x - view.x) / view.k, y: (m.y - view.y) / view.k };
  let best: { id: number; node: GraphNode; d: number } | null = null;
  for (const [id, node] of nodes) {
    const d = Math.hypot(node.x - g.x, node.y - g.y);
    if (d < PICK_RADIUS / view.k && (!best || d < best.d)) best = { id, node, d };
  }
  return best ? { id: best.id, node: best.node } : null;
}

function resizeRelCanvas(canvas: HTMLCanvasElement, size: { current: Size }) {
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  // Keep the true measured size (possibly 0 while the canvas hasn't been
  // laid out yet, or while it's hidden) so canRunFrame can tell a real
  // measurement apart from an unmeasured one. The backing bitmap still needs
  // at least 1 device pixel per side.
  const width = Math.round(rect.width);
  const height = Math.round(rect.height);
  canvas.width = Math.max(1, width) * dpr;
  canvas.height = Math.max(1, height) * dpr;
  size.current = { width, height, dpr };
}

function drawRel(
  ctx: CanvasRenderingContext2D,
  size: Size,
  layout: LayoutState,
  graph: GraphData,
  visible: Set<number>,
  view: ViewTransform,
  selected: number | null,
  hover: number | null,
) {
  const { width, height, dpr } = size;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  ctx.setTransform(dpr * view.k, 0, 0, dpr * view.k, dpr * view.x, dpr * view.y);

  const many = visible.size > MANY_THRESHOLD;

  ctx.strokeStyle = 'rgba(255,255,255,0.20)';
  ctx.lineWidth = 1.2;
  for (const edge of graph.edges) {
    if (!visible.has(edge.a) || !visible.has(edge.b)) continue;
    const a = layout.nodes.get(edge.a);
    const b = layout.nodes.get(edge.b);
    if (!a || !b) continue;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();
  }

  ctx.font = '10px "Fira Sans", sans-serif';
  for (const [id, n] of layout.nodes) {
    if (!visible.has(id)) continue;
    const kind = graph.kinds.get(id) ?? 'entity';
    const isSelected = id === selected;
    const isHovered = id === hover;
    ctx.beginPath();
    ctx.arc(n.x, n.y, isSelected ? 7 : 5, 0, Math.PI * 2);
    ctx.fillStyle = KIND_COLOR[kind];
    ctx.fill();
    if (isSelected || isHovered) {
      ctx.strokeStyle = isSelected ? '#206EC8' : 'rgba(255,255,255,0.6)';
      ctx.lineWidth = 2;
      ctx.stroke();
    }
    if (!many || isSelected || isHovered) {
      ctx.fillStyle = isSelected ? '#ECECEC' : '#A8A8A8';
      ctx.fillText(graph.names.get(id) ?? entityLabel(id), n.x + 9, n.y + 3);
    }
  }

  // hover tooltip: screen space, constant size regardless of zoom
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  if (hover !== null && visible.has(hover)) {
    const n = layout.nodes.get(hover);
    if (n) {
      const sx = n.x * view.k + view.x;
      const sy = n.y * view.k + view.y;
      const line = `${graph.names.get(hover) ?? 'Entity'}  ${entityLabel(hover)}`;
      ctx.font = '10px "JetBrains Mono", monospace';
      const w = ctx.measureText(line).width + 16;
      const x = Math.min(sx + 12, width - w - 6);
      const y = Math.max(sy - 30, 6);
      ctx.fillStyle = 'rgba(31,31,36,0.95)';
      ctx.strokeStyle = '#525252';
      ctx.beginPath();
      ctx.roundRect(x, y, w, 22, 4);
      ctx.fill();
      ctx.stroke();
      ctx.fillStyle = '#ECECEC';
      ctx.fillText(line, x + 8, y + 14);
    }
  }

  // zoom chip
  if (Math.abs(view.k - 1) > 0.02) {
    ctx.font = '9px "JetBrains Mono", monospace';
    ctx.fillStyle = 'rgba(31,31,36,0.85)';
    ctx.beginPath();
    ctx.roundRect(width - 52, height - 22, 44, 15, 3);
    ctx.fill();
    ctx.fillStyle = '#A8A8A8';
    ctx.fillText(`${Math.round(view.k * 100)}%`, width - 44, height - 11);
  }
}

async function copyEntityBsn(entity: number) {
  try {
    const { bsn } = await jackdaw.entityBsn(entity);
    await navigator.clipboard.writeText(bsn);
    toast('ok', 'jackdaw/entity_bsn: copied to clipboard');
  } catch (err) {
    toast('err', err instanceof Error ? err.message : String(err));
  }
}

export function RelationshipsTab() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sizeRef = useRef<Size>({ width: 0, height: 0, dpr: 1 });
  const layoutRef = useRef<LayoutState>({ nodes: new Map(), alpha: 1 });
  const viewRef = useRef<ViewTransform>({ x: 0, y: 0, k: 1 });
  const userNavigatedRef = useRef(false);
  const needsWarmRef = useRef(true);
  const hoverRef = useRef<number | null>(null);
  const dragRef = useRef<DragState | null>(null);
  const graphRef = useRef<GraphData>({ ids: [], parents: new Map(), edges: [], kinds: new Map(), names: new Map() });
  const filterRef = useRef('');
  const focusRef = useRef(false);

  const [filter, setFilter] = useState('');
  const [focus, setFocus] = useState(false);

  useEffect(() => {
    treePoll.start();
    return () => treePoll.stop();
  }, []);

  const rows = treePoll.data.value;
  const graph = useMemo(() => buildGraphData(rows ?? []), [rows]);
  graphRef.current = graph;
  filterRef.current = filter;
  focusRef.current = focus;

  const selected = selectedEntity.value;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      resizeRelCanvas(canvas, sizeRef);
      if (!userNavigatedRef.current && layoutRef.current.nodes.size && !needsWarmRef.current) {
        const visible = computeVisible(graphRef.current, filterRef.current, focusRef.current, selectedEntity.value);
        const pts = [...layoutRef.current.nodes].filter(([id]) => visible.has(id)).map(([, n]) => n);
        viewRef.current = fitView(pts, sizeRef.current.width, sizeRef.current.height);
      }
    });
    observer.observe(canvas);
    resizeRelCanvas(canvas, sizeRef);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext('2d') ?? null;
    let raf = 0;

    function frame() {
      if (canvas && ctx) {
        if (!sizeRef.current.width || !sizeRef.current.height) resizeRelCanvas(canvas, sizeRef);

        if (canRunFrame(sizeRef.current.width, sizeRef.current.height)) {
          const currentGraph = graphRef.current;
          const layout = layoutRef.current;

          let added = 0;
          for (const id of currentGraph.ids) if (!layout.nodes.has(id)) added++;
          syncNodes(layout, currentGraph.ids, currentGraph.parents, sizeRef.current.width, sizeRef.current.height);
          if (added > 5) needsWarmRef.current = true;

          const visible = computeVisible(currentGraph, filterRef.current, focusRef.current, selectedEntity.value);

          if (shouldWarm(needsWarmRef.current, currentGraph.ids.length)) {
            warmStart(layout, currentGraph.edges, visible, sizeRef.current.width, sizeRef.current.height);
            needsWarmRef.current = false;
            if (!userNavigatedRef.current) {
              viewRef.current = fitView(layout.nodes.values(), sizeRef.current.width, sizeRef.current.height);
            }
            step(layout, currentGraph.edges, visible, sizeRef.current.width, sizeRef.current.height);
          } else {
            step(layout, currentGraph.edges, visible, sizeRef.current.width, sizeRef.current.height);
          }

          drawRel(ctx, sizeRef.current, layout, currentGraph, visible, viewRef.current, selectedEntity.value, hoverRef.current);
        }
      }
      raf = requestAnimationFrame(frame);
    }
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, []);

  function screenPoint(ev: { clientX: number; clientY: number }, canvas: HTMLCanvasElement): Point {
    const rect = canvas.getBoundingClientRect();
    return { x: ev.clientX - rect.left, y: ev.clientY - rect.top };
  }

  function onPointerDown(ev: PointerEvent) {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.setPointerCapture(ev.pointerId);
    const m = screenPoint(ev, canvas);
    const hit = pickNode(m, viewRef.current, layoutRef.current.nodes);
    dragRef.current = hit ? { mode: 'node', id: hit.id, moved: false } : { mode: 'pan', last: m, moved: false };
    hoverRef.current = null;
  }

  function onPointerMove(ev: PointerEvent) {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const m = screenPoint(ev, canvas);
    const drag = dragRef.current;

    if (drag?.mode === 'pan') {
      viewRef.current = { ...viewRef.current, x: viewRef.current.x + (m.x - drag.last.x), y: viewRef.current.y + (m.y - drag.last.y) };
      drag.last = m;
      drag.moved = true;
      userNavigatedRef.current = true;
      canvas.style.cursor = 'grabbing';
      return;
    }
    if (drag?.mode === 'node') {
      const node = layoutRef.current.nodes.get(drag.id);
      if (node) {
        const g = { x: (m.x - viewRef.current.x) / viewRef.current.k, y: (m.y - viewRef.current.y) / viewRef.current.k };
        node.x = g.x;
        node.y = g.y;
      }
      drag.moved = true;
      layoutRef.current.alpha = Math.max(layoutRef.current.alpha, 0.25);
      return;
    }

    const hit = pickNode(m, viewRef.current, layoutRef.current.nodes);
    hoverRef.current = hit ? hit.id : null;
    canvas.style.cursor = hit ? 'pointer' : 'default';
  }

  function onPointerUp() {
    const canvas = canvasRef.current;
    const drag = dragRef.current;
    dragRef.current = null;
    if (canvas) canvas.style.cursor = 'default';
    if (drag?.mode === 'node' && !drag.moved) {
      selectedEntity.value = drag.id;
    }
  }

  function onDblClick(ev: MouseEvent) {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const m = screenPoint(ev, canvas);
    const hit = pickNode(m, viewRef.current, layoutRef.current.nodes);
    if (hit) {
      selectedEntity.value = hit.id;
      page.value = 'entities';
    }
  }

  function onWheel(ev: WheelEvent) {
    ev.preventDefault();
    const canvas = canvasRef.current;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const mx = ev.clientX - rect.left;
    const my = ev.clientY - rect.top;
    const view = viewRef.current;
    const factor = Math.pow(1.0015, -ev.deltaY);
    const k = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, view.k * factor));
    viewRef.current = {
      k,
      x: mx - (mx - view.x) * (k / view.k),
      y: my - (my - view.y) * (k / view.k),
    };
    userNavigatedRef.current = true;
  }

  function handleFit() {
    userNavigatedRef.current = false;
    const visible = computeVisible(graphRef.current, filterRef.current, focusRef.current, selectedEntity.value);
    const pts = [...layoutRef.current.nodes].filter(([id]) => visible.has(id)).map(([, n]) => n);
    viewRef.current = fitView(pts, sizeRef.current.width, sizeRef.current.height);
  }

  const parentId = selected !== null ? graph.parents.get(selected) ?? null : null;
  const childCount = selected !== null ? graph.ids.filter((id) => graph.parents.get(id) === selected).length : 0;
  const canBsn = capabilities.value.has('jackdaw/entity_bsn');

  return (
    <>
      <canvas
        ref={canvasRef}
        class="rel-canvas"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onDblClick={onDblClick}
        onWheel={onWheel}
      />
      <div class="rel-toolbar">
        <div class="search-box" style="width:180px">
          <span class="icon">
            <Search />
          </span>
          <input
            placeholder="Filter nodes"
            spellcheck={false}
            value={filter}
            onInput={(ev) => setFilter((ev.target as HTMLInputElement).value)}
          />
        </div>
        <button
          class={`vp-btn${focus ? ' active' : ''}`}
          title="Show only the selection and its neighbors (2 hops)"
          onClick={() => setFocus((f) => !f)}
        >
          <Icon of={CircleDot} />
          Focus
        </button>
        <button class="vp-btn" title="Recenter the graph" onClick={handleFit}>
          <Icon of={RotateCw} />
          Fit
        </button>
      </div>
      {selected !== null && graph.names.has(selected) && (
        <div class="rel-info on">
          <div class="ri-title">
            <span class="kind-dot" style={`background:var(--dot-${graph.kinds.get(selected) ?? 'entity'})`} />
            <span>{graph.names.get(selected) ?? entityLabel(selected)}</span>
          </div>
          <div class="ri-id">
            {entityLabel(selected)} &middot; {entityPath(selected, graph.parents, graph.names)}
          </div>
          <div class="ri-rels">
            {parentId !== null && <span>parent <b>{graph.names.get(parentId) ?? entityLabel(parentId)}</b></span>}
            <span>children <b>{childCount}</b></span>
          </div>
          <div class="ri-actions">
            <button
              class="btn-ghost"
              onClick={() => {
                page.value = 'entities';
              }}
            >
              <Icon of={Boxes} />
              Open in Entities
            </button>
            {canBsn && (
              <button
                class="btn-ghost"
                onClick={() => {
                  void copyEntityBsn(selected);
                }}
              >
                <Icon of={Braces} />
                Copy BSN
              </button>
            )}
          </div>
        </div>
      )}
      <div class="rel-legend">
        <div class="row">
          <span class="line" />
          ChildOf (hierarchy)
        </div>
        <div class="note">Custom relationships need registry support</div>
      </div>
      <div class="rel-hint">drag nodes &middot; drag background pan &middot; wheel zoom &middot; click select &middot; double-click opens in Entities</div>
    </>
  );
}
