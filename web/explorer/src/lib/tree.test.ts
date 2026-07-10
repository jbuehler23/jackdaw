import { describe, expect, it } from 'vitest';
import { assembleTree } from './tree';

const row = (entity: number, name: string | null, parent: number | null) => ({
  entity,
  components: {
    ...(name !== null ? { 'bevy_ecs::name::Name': name } : {}),
    ...(parent !== null ? { 'bevy_ecs::hierarchy::ChildOf': parent } : {}),
  },
  has: {},
});

describe('assembleTree', () => {
  it('nests children under parents and keeps roots sorted by entity id', () => {
    const tree = assembleTree([row(3, 'Child', 1), row(1, 'Root', null), row(2, 'Other', null)]);
    expect(tree.map((n) => n.entity)).toEqual([1, 2]);
    expect(tree[0].children[0].entity).toBe(3);
  });

  it('treats a child of a missing parent as a root (orphan)', () => {
    const tree = assembleTree([row(5, 'Orphan', 99)]);
    expect(tree.map((n) => n.entity)).toEqual([5]);
  });

  it('reads names and leaves null when absent', () => {
    const tree = assembleTree([row(1, null, null)]);
    expect(tree[0].name).toBeNull();
  });
});
