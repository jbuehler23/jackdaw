// ViewportPage.tsx: canvas 3D viewport (orbit camera, picking, translate gizmo,
// Add menu). All camera/projection math lives in lib/viewport-math; this
// component only wires it to the canvas and pointer events.
import { useEffect, useRef, useState } from 'preact/hooks';
import { Diamond, Lightbulb, Plus, Sun, Zap } from 'lucide-preact';
import { Icon } from './Icon';
import { world } from '../lib/brp';
import { loadRegistry } from '../lib/registry';
import { DIRECTIONAL_LIGHT, NAME, POINT_LIGHT, SPOT_LIGHT, TRANSFORM } from '../lib/tree';
import { buildScene, viewportPoll, type SceneItem, type Vec3 } from '../lib/viewport-scene';
import { treePoll } from '../lib/treeData';
import { selectedEntity } from '../lib/state';
import { toast } from '../lib/toasts';
import {
  axisDragDelta,
  axisGizmo,
  boxFaces,
  camBasis,
  orbit,
  pan,
  project,
  zoom,
  type CameraState,
  type GizmoAxis,
} from '../lib/viewport-math';

const BOX_HALF: Vec3 = { x: 0.5, y: 0.5, z: 0.5 };
const BOX_BASE = '#63666E';
const GIZMO_COLORS: Record<'x' | 'y' | 'z', string> = { x: '#AB4051', y: '#5D8D0A', z: '#2160A3' };
// Precomputed cos/sin of the arrowhead's 2.5 radian spread, so the gizmo arrow
// is drawn with plain vector rotation instead of calling Math.cos/sin here.
const ARROW_COS = -0.8011436155469337;
const ARROW_SIN = 0.5984721441039565;

type Point = { x: number; y: number };

type DragState =
  | { mode: 'orbit' | 'pan'; last: Point; down: Point; moved: boolean }
  | { mode: 'axis'; entity: number; axis: GizmoAxis; delta: number; last: Point };

interface Pickable {
  entity: number;
  x: number;
  y: number;
  z: number;
}

interface Size {
  width: number;
  height: number;
  dpr: number;
}

function shade(hex: string, k: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgb(${Math.round(r * k)},${Math.round(g * k)},${Math.round(b * k)})`;
}

function distToSegment(p: Point, a: Point, b: Point): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const lenSq = dx * dx + dy * dy || 1;
  const t = Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / lenSq));
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

function boxCorners(c: Vec3, h: Vec3): Vec3[] {
  const out: Vec3[] = [];
  for (const dy of [-1, 1]) {
    for (const dz of [-1, 1]) {
      for (const dx of dz === -1 ? [-1, 1] : [1, -1]) {
        out.push({ x: c.x + dx * h.x, y: c.y + dy * h.y, z: c.z + dz * h.z });
      }
    }
  }
  return out;
}

const BOX_EDGES: [number, number][] = [
  [0, 1], [1, 2], [2, 3], [3, 0],
  [4, 5], [5, 6], [6, 7], [7, 4],
  [0, 4], [1, 5], [2, 6], [3, 7],
];

// Renders the axis drag by nudging the drawn (world) position by the
// accumulated delta, without touching the entity's local Transform value.
function overridePos(item: SceneItem, drag: DragState | null): Vec3 {
  if (drag && drag.mode === 'axis' && drag.entity === item.entity) {
    const axis = drag.axis.axis;
    return { ...item.pos, [axis]: item.pos[axis] + drag.delta };
  }
  return item.pos;
}

// Transform.translation is local: the commit value is the entity's own
// (pre-drag) local translation plus the accumulated drag delta, not the
// world-derived pos used for drawing.
export function commitValueFor(item: SceneItem, axis: 'x' | 'y' | 'z', delta: number): number {
  return item.localTranslation[axis] + delta;
}

function resizeCanvas(canvas: HTMLCanvasElement, size: { current: Size }) {
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(rect.width));
  const height = Math.max(1, Math.round(rect.height));
  canvas.width = width * dpr;
  canvas.height = height * dpr;
  size.current = { width, height, dpr };
}

function drawFrame(
  ctx: CanvasRenderingContext2D,
  size: Size,
  camera: CameraState,
  items: SceneItem[],
  selected: number | null,
  drag: DragState | null,
): { gizmo: GizmoAxis[]; pickables: Pickable[] } {
  const { width, height, dpr } = size;
  const basis = camBasis(camera);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);

  // ground grid on y=0
  ctx.lineWidth = 1;
  for (let i = -24; i <= 24; i += 2) {
    const major = i % 8 === 0;
    ctx.strokeStyle = major ? 'rgba(255,255,255,0.10)' : 'rgba(255,255,255,0.045)';
    const a = project({ x: i, y: 0, z: -24 }, basis, width, height);
    const b = project({ x: i, y: 0, z: 24 }, basis, width, height);
    if (a && b) {
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
    }
    const c = project({ x: -24, y: 0, z: i }, basis, width, height);
    const d = project({ x: 24, y: 0, z: i }, basis, width, height);
    if (c && d) {
      ctx.beginPath();
      ctx.moveTo(c.x, c.y);
      ctx.lineTo(d.x, d.y);
      ctx.stroke();
    }
  }

  // origin x/z axis lines
  const axisLines: { end: Vec3; color: string }[] = [
    { end: { x: 8, y: 0.01, z: 0 }, color: '#AB4051' },
    { end: { x: 0, y: 0.01, z: 8 }, color: '#2160A3' },
  ];
  for (const line of axisLines) {
    const o = project({ x: 0, y: 0.01, z: 0 }, basis, width, height);
    const p = project(line.end, basis, width, height);
    if (o && p) {
      ctx.strokeStyle = line.color;
      ctx.globalAlpha = 0.6;
      ctx.beginPath();
      ctx.moveTo(o.x, o.y);
      ctx.lineTo(p.x, p.y);
      ctx.stroke();
      ctx.globalAlpha = 1;
    }
  }

  const faces: { depth: number; pts: { x: number; y: number }[]; shade: number }[] = [];
  const bills: { kind: 'light' | 'camera' | 'marker'; x: number; y: number; z: number }[] = [];
  const pickables: Pickable[] = [];

  for (const item of items) {
    const pos = overridePos(item, drag);
    const sp = project(pos, basis, width, height);
    if (sp) pickables.push({ entity: item.entity, x: sp.x, y: sp.y, z: sp.z });
    if (item.kind === 'box') {
      faces.push(...boxFaces(pos, BOX_HALF, basis, width, height));
    } else if (sp) {
      bills.push({ kind: item.kind, x: sp.x, y: sp.y, z: sp.z });
    }
  }

  faces.sort((a, b) => b.depth - a.depth);
  for (const f of faces) {
    ctx.beginPath();
    ctx.moveTo(f.pts[0].x, f.pts[0].y);
    for (const p of f.pts.slice(1)) ctx.lineTo(p.x, p.y);
    ctx.closePath();
    ctx.fillStyle = shade(BOX_BASE, f.shade);
    ctx.fill();
    ctx.strokeStyle = 'rgba(0,0,0,0.28)';
    ctx.lineWidth = 1;
    ctx.stroke();
  }

  bills.sort((a, b) => b.z - a.z);
  for (const b of bills) {
    const { x, y, z } = b;
    if (b.kind === 'light') {
      const r = Math.max(5, (9 * 24) / z);
      const g = ctx.createRadialGradient(x, y, 1, x, y, r * 2.2);
      g.addColorStop(0, 'rgba(255,190,100,0.85)');
      g.addColorStop(1, 'rgba(255,190,100,0)');
      ctx.beginPath();
      ctx.arc(x, y, r * 2.2, 0, Math.PI * 2);
      ctx.fillStyle = g;
      ctx.fill();
      ctx.beginPath();
      ctx.arc(x, y, r * 0.45, 0, Math.PI * 2);
      ctx.fillStyle = 'rgba(255,190,100,1)';
      ctx.fill();
    } else if (b.kind === 'camera') {
      const s = Math.max(6, (10 * 24) / z);
      ctx.strokeStyle = '#4A9BDB';
      ctx.lineWidth = 1.5;
      ctx.strokeRect(x - s, y - s * 0.6, s * 1.2, s * 1.2);
      ctx.beginPath();
      ctx.moveTo(x + s * 0.2, y - s * 0.2);
      ctx.lineTo(x + s, y - s * 0.6);
      ctx.lineTo(x + s, y + s * 0.6);
      ctx.lineTo(x + s * 0.2, y + s * 0.2);
      ctx.closePath();
      ctx.stroke();
    } else {
      const s = Math.max(4, (6 * 24) / z);
      ctx.strokeStyle = '#8A8A90';
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x, y - s);
      ctx.lineTo(x + s, y);
      ctx.lineTo(x, y + s);
      ctx.lineTo(x - s, y);
      ctx.closePath();
      ctx.stroke();
    }
  }

  const selectedItem = selected != null ? items.find((i) => i.entity === selected) : undefined;
  let gizmo: GizmoAxis[] = [];
  if (selectedItem) {
    const pos = overridePos(selectedItem, drag);
    if (selectedItem.kind === 'box') {
      const corners = boxCorners(pos, BOX_HALF).map((p) => project(p, basis, width, height));
      if (corners.every((c) => c !== null)) {
        ctx.strokeStyle = '#206EC8';
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        for (const [a, bEdge] of BOX_EDGES) {
          const pa = corners[a];
          const pb = corners[bEdge];
          if (!pa || !pb) continue;
          ctx.moveTo(pa.x, pa.y);
          ctx.lineTo(pb.x, pb.y);
        }
        ctx.stroke();
      }
    }

    const len = camera.dist * 0.09;
    gizmo = axisGizmo(pos, len, basis, width, height);
    for (const axis of gizmo) {
      const hot = drag?.mode === 'axis' && drag.axis.axis === axis.axis;
      const color = hot ? '#FFFFFF' : GIZMO_COLORS[axis.axis];
      ctx.strokeStyle = color;
      ctx.lineWidth = hot ? 3 : 2;
      ctx.beginPath();
      ctx.moveTo(axis.o.x, axis.o.y);
      ctx.lineTo(axis.tip.x, axis.tip.y);
      ctx.stroke();

      const dx = axis.tip.x - axis.o.x;
      const dy = axis.tip.y - axis.o.y;
      const len2 = Math.hypot(dx, dy) || 1;
      const ux = dx / len2;
      const uy = dy / len2;
      const tip1 = { x: axis.tip.x + ux * 9, y: axis.tip.y + uy * 9 };
      const back1 = { x: ux * ARROW_COS - uy * ARROW_SIN, y: ux * ARROW_SIN + uy * ARROW_COS };
      const back2 = { x: ux * ARROW_COS - uy * -ARROW_SIN, y: ux * -ARROW_SIN + uy * ARROW_COS };
      const tip2 = { x: axis.tip.x + back1.x * 6, y: axis.tip.y + back1.y * 6 };
      const tip3 = { x: axis.tip.x + back2.x * 6, y: axis.tip.y + back2.y * 6 };
      ctx.beginPath();
      ctx.moveTo(tip1.x, tip1.y);
      ctx.lineTo(tip2.x, tip2.y);
      ctx.lineTo(tip3.x, tip3.y);
      ctx.closePath();
      ctx.fillStyle = color;
      ctx.fill();
    }
  }

  return { gizmo, pickables };
}

function screenPoint(ev: PointerEvent, canvas: HTMLCanvasElement): Point {
  const rect = canvas.getBoundingClientRect();
  return { x: ev.clientX - rect.left, y: ev.clientY - rect.top };
}

function transformAt(target: Vec3): Record<string, unknown> {
  return {
    translation: { x: target.x, y: target.y, z: target.z },
    rotation: { x: 0, y: 0, z: 0, w: 1 },
    scale: { x: 1, y: 1, z: 1 },
  };
}

export function ViewportPage() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const cameraRef = useRef<CameraState>({ target: { x: 0, y: 1, z: 0 }, yaw: 0.6, pitch: 0.55, dist: 30 });
  const sizeRef = useRef<Size>({ width: 0, height: 0, dpr: 1 });
  const dragRef = useRef<DragState | null>(null);
  const gizmoRef = useRef<GizmoAxis[]>([]);
  const pickablesRef = useRef<Pickable[]>([]);
  const itemsRef = useRef<SceneItem[]>([]);
  const [menuOpen, setMenuOpen] = useState(false);

  const rows = viewportPoll.data.value;
  itemsRef.current = buildScene(rows ?? []);

  useEffect(() => {
    viewportPoll.start();
    return () => viewportPoll.stop();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => resizeCanvas(canvas, sizeRef));
    observer.observe(canvas);
    resizeCanvas(canvas, sizeRef);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext('2d') ?? null;
    let raf = 0;

    function frame() {
      if (ctx) {
        const { gizmo, pickables } = drawFrame(
          ctx,
          sizeRef.current,
          cameraRef.current,
          itemsRef.current,
          selectedEntity.value,
          dragRef.current,
        );
        gizmoRef.current = gizmo;
        pickablesRef.current = pickables;
      }
      raf = requestAnimationFrame(frame);
    }
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, []);

  useEffect(() => {
    function onKeyDown(ev: KeyboardEvent) {
      if (ev.key.toLowerCase() !== 'f') return;
      const target = ev.target as HTMLElement | null;
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
      const entity = selectedEntity.value;
      const item = entity != null ? itemsRef.current.find((i) => i.entity === entity) : undefined;
      if (!item) return;
      cameraRef.current = { ...cameraRef.current, target: item.pos, dist: Math.min(cameraRef.current.dist, 16) };
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, []);

  useEffect(() => {
    function onDocClick() {
      setMenuOpen(false);
    }
    document.addEventListener('click', onDocClick);
    return () => document.removeEventListener('click', onDocClick);
  }, []);

  function onPointerDown(ev: PointerEvent) {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.setPointerCapture(ev.pointerId);
    const m = screenPoint(ev, canvas);

    const entity = selectedEntity.value;
    const item = entity != null ? itemsRef.current.find((i) => i.entity === entity) : undefined;
    if (item) {
      for (const axis of gizmoRef.current) {
        if (distToSegment(m, axis.o, axis.tip) < 9) {
          dragRef.current = { mode: 'axis', entity: item.entity, axis, delta: 0, last: m };
          return;
        }
      }
    }

    dragRef.current = { mode: ev.shiftKey || ev.button === 1 ? 'pan' : 'orbit', last: m, down: m, moved: false };
  }

  function onPointerMove(ev: PointerEvent) {
    const canvas = canvasRef.current;
    const drag = dragRef.current;
    if (!canvas || !drag) return;
    const m = screenPoint(ev, canvas);

    if (drag.mode === 'axis') {
      const dx = m.x - drag.last.x;
      const dy = m.y - drag.last.y;
      drag.last = m;
      drag.delta += axisDragDelta(drag.axis, dx, dy);
      return;
    }

    const dx = m.x - drag.last.x;
    const dy = m.y - drag.last.y;
    drag.last = m;
    if (Math.hypot(m.x - drag.down.x, m.y - drag.down.y) > 3) drag.moved = true;
    if (drag.mode === 'orbit') {
      cameraRef.current = orbit(cameraRef.current, dx, dy);
    } else {
      cameraRef.current = pan(cameraRef.current, camBasis(cameraRef.current), dx, dy);
    }
  }

  function onPointerUp(ev: PointerEvent) {
    const drag = dragRef.current;
    dragRef.current = null;
    if (!drag) return;

    if (drag.mode === 'axis') {
      const item = itemsRef.current.find((i) => i.entity === drag.entity);
      const axis = drag.axis.axis;
      if (item) {
        const value = commitValueFor(item, axis, drag.delta);
        world
          .mutateComponents(drag.entity, TRANSFORM, `translation.${axis}`, value)
          .then(() => {
            toast('ok', `world.mutate_components: Transform.translation.${axis}`);
            void viewportPoll.refresh();
          })
          .catch((err) => {
            toast('err', err instanceof Error ? err.message : String(err));
          });
      }
      return;
    }

    if (!drag.moved) {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const m = screenPoint(ev, canvas);
      let best: { entity: number; z: number } | null = null;
      for (const p of pickablesRef.current) {
        const d = Math.hypot(p.x - m.x, p.y - m.y);
        if (d < 18 && (!best || p.z < best.z)) best = { entity: p.entity, z: p.z };
      }
      if (best) selectedEntity.value = best.entity;
    }
  }

  function onWheel(ev: WheelEvent) {
    ev.preventDefault();
    cameraRef.current = zoom(cameraRef.current, ev.deltaY);
  }

  async function spawnEmpty() {
    try {
      const entity = await world.spawnEntity({});
      await world.insertComponents(entity, { [NAME]: 'Entity' });
      selectedEntity.value = entity;
      toast('ok', 'world.spawn_entity + world.insert_components: Empty entity');
      void viewportPoll.refresh();
      void treePoll.refresh();
    } catch (err) {
      toast('err', err instanceof Error ? err.message : String(err));
    }
    setMenuOpen(false);
  }

  async function spawnLight(componentPath: string, label: string) {
    try {
      const registry = await loadRegistry();
      const schema = registry.get(componentPath);
      const entity = await world.spawnEntity({});
      await world.insertComponents(entity, {
        [TRANSFORM]: transformAt(cameraRef.current.target),
        [componentPath]: schema ? schema.defaultValue() : {},
      });
      selectedEntity.value = entity;
      toast('ok', `world.spawn_entity + world.insert_components: ${label}`);
      void viewportPoll.refresh();
      void treePoll.refresh();
    } catch (err) {
      toast('err', err instanceof Error ? err.message : String(err));
    }
    setMenuOpen(false);
  }

  return (
    <div class="pane viewport-pane" style="flex:1;">
      <canvas
        ref={canvasRef}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onWheel={onWheel}
      />
      <div class="vp-toolbar">
        <button
          class="vp-btn"
          onClick={(ev) => {
            ev.stopPropagation();
            setMenuOpen((open) => !open);
          }}
        >
          <Icon of={Plus} />
          Add
        </button>
        <div class={`add-menu${menuOpen ? ' open' : ''}`}>
          <div class="group">Other</div>
          <button
            onClick={() => {
              void spawnEmpty();
            }}
          >
            <Icon of={Diamond} />
            Empty entity
          </button>
          <div class="group">Light</div>
          <button
            onClick={() => {
              void spawnLight(POINT_LIGHT, 'Point light');
            }}
          >
            <Icon of={Lightbulb} />
            Point light
          </button>
          <button
            onClick={() => {
              void spawnLight(SPOT_LIGHT, 'Spot light');
            }}
          >
            <Icon of={Zap} />
            Spot light
          </button>
          <button
            onClick={() => {
              void spawnLight(DIRECTIONAL_LIGHT, 'Directional light');
            }}
          >
            <Icon of={Sun} />
            Directional light
          </button>
          <div class="group" style="opacity:0.7">
            Meshes need game-side assets
          </div>
        </div>
      </div>
      <div class="vp-hint">drag orbit &middot; shift-drag pan &middot; wheel zoom &middot; click select &middot; drag axes to move &middot; F frame</div>
    </div>
  );
}
