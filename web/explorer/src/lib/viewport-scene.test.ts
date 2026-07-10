import { describe, expect, it } from 'vitest';
import { CAMERA, CHILD_OF, DIRECTIONAL_LIGHT, MESH3D, NAME, TRANSFORM } from './tree';
import { buildScene, type ViewportRow } from './viewport-scene';

function row(
  entity: number,
  opts: {
    translation?: { x: number; y: number; z: number } | [number, number, number];
    parent?: number | { parent: number } | [number];
    name?: string;
    has?: Partial<Record<string, boolean>>;
  } = {},
): ViewportRow {
  const components: Record<string, unknown> = {};
  if (opts.translation) components[TRANSFORM] = { translation: opts.translation, rotation: {}, scale: {} };
  if (opts.parent !== undefined) components[CHILD_OF] = opts.parent;
  if (opts.name !== undefined) components[NAME] = opts.name;
  return { entity, components, has: (opts.has ?? {}) as Record<string, boolean> };
}

describe('buildScene', () => {
  it('sums translations up the ChildOf chain to compute world position', () => {
    const rows = [
      row(1, { translation: { x: 10, y: 0, z: 0 } }),
      row(2, { translation: { x: 1, y: 2, z: 3 }, parent: 1 }),
    ];
    const scene = buildScene(rows);
    const child = scene.find((s) => s.entity === 2)!;
    expect(child.pos).toEqual({ x: 11, y: 2, z: 3 });
  });

  it('keeps localTranslation as the entity own Transform value, distinct from summed world pos', () => {
    const rows = [
      row(1, { translation: { x: 10, y: 0, z: 0 } }),
      row(2, { translation: { x: 1, y: 2, z: 3 }, parent: 1 }),
    ];
    const scene = buildScene(rows);
    const child = scene.find((s) => s.entity === 2)!;
    expect(child.pos).toEqual({ x: 11, y: 2, z: 3 });
    expect(child.localTranslation).toEqual({ x: 1, y: 2, z: 3 });
  });

  it('handles array-shaped translations and array/object ChildOf wire shapes', () => {
    const rows = [
      row(1, { translation: [5, 0, 0] }),
      row(2, { translation: [1, 1, 1], parent: { parent: 1 } }),
      row(3, { translation: [1, 1, 1], parent: [1] }),
    ];
    const scene = buildScene(rows);
    expect(scene.find((s) => s.entity === 2)!.pos).toEqual({ x: 6, y: 1, z: 1 });
    expect(scene.find((s) => s.entity === 3)!.pos).toEqual({ x: 6, y: 1, z: 1 });
    expect(scene.find((s) => s.entity === 2)!.localTranslation).toEqual({ x: 1, y: 1, z: 1 });
    expect(scene.find((s) => s.entity === 3)!.localTranslation).toEqual({ x: 1, y: 1, z: 1 });
  });

  it('skips entities without a Transform', () => {
    const rows = [row(1, {}), row(2, { translation: { x: 0, y: 0, z: 0 } })];
    const scene = buildScene(rows);
    expect(scene.map((s) => s.entity)).toEqual([2]);
  });

  it('is cycle-safe when ChildOf forms a loop', () => {
    const rows = [
      row(1, { translation: { x: 1, y: 0, z: 0 }, parent: 2 }),
      row(2, { translation: { x: 1, y: 0, z: 0 }, parent: 1 }),
    ];
    expect(() => buildScene(rows)).not.toThrow();
    const scene = buildScene(rows);
    expect(scene.length).toBe(2);
  });

  it('classifies kind with priority camera > light > box > marker', () => {
    const rows = [
      row(1, { translation: { x: 0, y: 0, z: 0 }, has: { [CAMERA]: true, [MESH3D]: true } }),
      row(2, { translation: { x: 0, y: 0, z: 0 }, has: { [DIRECTIONAL_LIGHT]: true, [MESH3D]: true } }),
      row(3, { translation: { x: 0, y: 0, z: 0 }, has: { [MESH3D]: true } }),
      row(4, { translation: { x: 0, y: 0, z: 0 } }),
    ];
    const scene = buildScene(rows);
    expect(scene.find((s) => s.entity === 1)!.kind).toBe('camera');
    expect(scene.find((s) => s.entity === 2)!.kind).toBe('light');
    expect(scene.find((s) => s.entity === 3)!.kind).toBe('box');
    expect(scene.find((s) => s.entity === 4)!.kind).toBe('marker');
  });

  it('reads the entity name when present', () => {
    const rows = [row(1, { translation: { x: 0, y: 0, z: 0 }, name: 'Hero' })];
    expect(buildScene(rows)[0].name).toBe('Hero');
  });

  it('defaults name to null when absent', () => {
    const rows = [row(1, { translation: { x: 0, y: 0, z: 0 } })];
    expect(buildScene(rows)[0].name).toBeNull();
  });
});
