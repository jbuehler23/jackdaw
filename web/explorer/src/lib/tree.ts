// tree.ts: assembles world.query rows into a parent/child entity tree.
//
// Wire-shape conclusions (checked against the bevy_ecs / bevy_remote source):
// - Name is bevy_ecs::name::Name (crates/bevy_ecs/src/name.rs). It derives
//   Serialize/Deserialize as a transparent newtype, so it comes back as a bare
//   string.
// - ChildOf is bevy_ecs::hierarchy::ChildOf(pub Entity)
//   (crates/bevy_ecs/src/hierarchy.rs), a single-field tuple struct deriving
//   Serialize/Deserialize. Serde's newtype-struct serialization forwards to the
//   inner value, and Entity's own Serialize impl emits `serialize_u64(to_bits())`
//   (crates/bevy_ecs/src/entity/mod.rs), so on the wire ChildOf is a bare number:
//   the same entity-bits value that appears in a row's `entity` field. `{parent:
//   n}` and `[n]` shapes are handled defensively below in case a server reflects
//   it differently, but the bare-number shape is what a live bevy_remote sends.
// - world.query's `data.has` is a plain string array (bevy_remote's BrpQuery.has:
//   Vec<String>), and each row's `has` comes back as Record<typePath, boolean>
//   (BrpQueryRow.has: HashMap<String, Value>, values are JSON booleans).

import { world, type QueryRow } from './brp';

export const NAME = 'bevy_ecs::name::Name';
export const CHILD_OF = 'bevy_ecs::hierarchy::ChildOf';
export const CAMERA = 'bevy_camera::components::Camera3d';
export const POINT_LIGHT = 'bevy_light::point_light::PointLight';
export const DIRECTIONAL_LIGHT = 'bevy_light::directional_light::DirectionalLight';
export const MESH3D = 'bevy_mesh::components::Mesh3d';
export const WORLD_ASSET_ROOT = 'bevy_world_serialization::components::WorldAssetRoot';

const HAS_PATHS = [CAMERA, POINT_LIGHT, DIRECTIONAL_LIGHT, MESH3D, WORLD_ASSET_ROOT];

export type EntityKind = 'camera' | 'light' | 'mesh' | 'prefab' | 'entity';

export interface TreeNode {
  entity: number;
  name: string | null;
  kind: EntityKind;
  parent: number | null;
  children: TreeNode[];
}

function parentEntity(value: unknown): number | null {
  if (typeof value === 'number') return value;
  if (Array.isArray(value) && typeof value[0] === 'number') return value[0];
  if (value && typeof value === 'object') {
    const parent = (value as { parent?: unknown }).parent;
    if (typeof parent === 'number') return parent;
  }
  return null;
}

export function classifyKind(has: Record<string, boolean> | undefined): EntityKind {
  const h = has ?? {};
  if (h[CAMERA]) return 'camera';
  if (h[POINT_LIGHT] || h[DIRECTIONAL_LIGHT]) return 'light';
  if (h[WORLD_ASSET_ROOT]) return 'prefab';
  if (h[MESH3D]) return 'mesh';
  return 'entity';
}

export function assembleTree(rows: QueryRow[]): TreeNode[] {
  const nodes = new Map<number, TreeNode>();
  for (const row of rows) {
    const name = row.components[NAME];
    nodes.set(row.entity, {
      entity: row.entity,
      name: typeof name === 'string' ? name : null,
      kind: classifyKind(row.has),
      parent: null,
      children: [],
    });
  }

  const roots: TreeNode[] = [];
  for (const row of rows) {
    const node = nodes.get(row.entity);
    if (!node) continue;
    const parent = parentEntity(row.components[CHILD_OF]);
    const parentNode = parent !== null ? nodes.get(parent) : undefined;
    if (parentNode) {
      node.parent = parentNode.entity;
      parentNode.children.push(node);
    } else {
      roots.push(node);
    }
  }

  const byEntity = (a: TreeNode, b: TreeNode) => a.entity - b.entity;
  roots.sort(byEntity);
  for (const node of nodes.values()) node.children.sort(byEntity);
  return roots;
}

export function fetchTreeRows(): Promise<QueryRow[]> {
  return world.query({
    data: { option: [NAME, CHILD_OF], has: HAS_PATHS },
    filter: {},
  });
}
