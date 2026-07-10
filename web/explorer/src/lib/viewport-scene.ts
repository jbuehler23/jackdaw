// viewport-scene.ts: turns polled world.query rows into renderable scene items
// for the canvas viewport. Pure data shaping; no canvas, no DOM.

import { createPoll, type Poll } from './poll';
import { world, type QueryRow } from './brp';
import { CAMERA, CHILD_OF, DIRECTIONAL_LIGHT, HAS_PATHS, MESH3D, NAME, POINT_LIGHT, SPOT_LIGHT, TRANSFORM } from './tree';

export type ViewportRow = QueryRow;

export type SceneItemKind = 'box' | 'light' | 'camera' | 'marker';

export interface Vec3 {
  x: number;
  y: number;
  z: number;
}

export interface SceneItem {
  entity: number;
  kind: SceneItemKind;
  pos: Vec3;
  name: string | null;
}

const MAX_CHAIN_DEPTH = 64;

function parentEntity(value: unknown): number | null {
  if (typeof value === 'number') return value;
  if (Array.isArray(value) && typeof value[0] === 'number') return value[0];
  if (value && typeof value === 'object') {
    const parent = (value as { parent?: unknown }).parent;
    if (typeof parent === 'number') return parent;
  }
  return null;
}

function translationOf(value: unknown): Vec3 | null {
  if (Array.isArray(value)) {
    const [x, y, z] = value as [number, number, number];
    if (typeof x === 'number' && typeof y === 'number' && typeof z === 'number') return { x, y, z };
    return null;
  }
  if (value && typeof value === 'object') {
    const { x, y, z } = value as Partial<Vec3>;
    if (typeof x === 'number' && typeof y === 'number' && typeof z === 'number') return { x, y, z };
  }
  return null;
}

function localTranslation(row: ViewportRow): Vec3 | null {
  const transform = row.components[TRANSFORM];
  if (!transform || typeof transform !== 'object') return null;
  return translationOf((transform as { translation?: unknown }).translation);
}

function classifyKind(has: Record<string, boolean> | undefined): SceneItemKind {
  const h = has ?? {};
  if (h[CAMERA]) return 'camera';
  if (h[POINT_LIGHT] || h[DIRECTIONAL_LIGHT] || h[SPOT_LIGHT]) return 'light';
  if (h[MESH3D]) return 'box';
  return 'marker';
}

export function buildScene(rows: ViewportRow[]): SceneItem[] {
  const byEntity = new Map<number, ViewportRow>();
  for (const row of rows) byEntity.set(row.entity, row);

  const items: SceneItem[] = [];
  for (const row of rows) {
    const local = localTranslation(row);
    if (!local) continue; // no Transform: nothing to place in the viewport

    let pos: Vec3 = { ...local };
    let current: ViewportRow | undefined = row;
    const visited = new Set<number>([row.entity]);
    for (let depth = 0; depth < MAX_CHAIN_DEPTH; depth++) {
      const parentId = parentEntity(current?.components[CHILD_OF]);
      if (parentId === null || visited.has(parentId)) break;
      const parentRow = byEntity.get(parentId);
      if (!parentRow) break;
      const parentLocal = localTranslation(parentRow);
      if (parentLocal) pos = { x: pos.x + parentLocal.x, y: pos.y + parentLocal.y, z: pos.z + parentLocal.z };
      visited.add(parentId);
      current = parentRow;
    }

    const name = row.components[NAME];
    items.push({
      entity: row.entity,
      kind: classifyKind(row.has),
      pos,
      name: typeof name === 'string' ? name : null,
    });
  }
  return items;
}

function fetchViewportRows(): Promise<QueryRow[]> {
  return world.query({
    data: { option: [TRANSFORM, NAME, CHILD_OF], has: HAS_PATHS },
    filter: {},
  });
}

export const viewportPoll: Poll<QueryRow[]> = createPoll(fetchViewportRows, 500);
