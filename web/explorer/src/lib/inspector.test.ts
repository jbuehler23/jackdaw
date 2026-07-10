import { describe, expect, it } from 'vitest';
import { commitPath, flattenFields, scrubValue, tupleWireShape } from './inspector';
import type { ComponentSchema } from './registry';

describe('scrubValue', () => {
  it('moves by step per pixel, x10 with shift', () => {
    expect(scrubValue(1, 10, 0.1, false)).toBeCloseTo(2);
    expect(scrubValue(1, 10, 0.1, true)).toBeCloseTo(11);
  });
});

describe('commitPath', () => {
  it('sets nested paths immutably', () => {
    const v = { translation: { x: 1, y: 2, z: 3 } };
    const next = commitPath(v, 'translation.x', 9) as typeof v;
    expect(next.translation.x).toBe(9);
    expect(v.translation.x).toBe(1);
  });
  it('empty path replaces the value', () => {
    expect(commitPath('Inherited', '', 'Hidden')).toBe('Hidden');
  });
});

describe('flattenFields', () => {
  const transform: ComponentSchema = {
    typePath: 'bevy_transform::components::transform::Transform',
    shortName: 'Transform',
    fields: [
      { name: 'translation', kind: 'vec3' },
      { name: 'scale', kind: 'vec3' },
    ],
    defaultValue: () => ({}),
  };

  it('addresses struct fields by their own dotted path', () => {
    const value = { translation: { x: 1, y: 2, z: 3 }, scale: { x: 1, y: 1, z: 1 } };
    const rows = flattenFields(transform, value);
    expect(rows).toEqual([
      { label: 'translation', binding: { component: transform.typePath, path: 'translation', kind: 'vec3' }, value: value.translation, options: undefined },
      { label: 'scale', binding: { component: transform.typePath, path: 'scale', kind: 'vec3' }, value: value.scale, options: undefined },
    ]);
  });

  it('addresses a unit enum and a single-field tuple struct with the empty path', () => {
    const visibility: ComponentSchema = {
      typePath: 'bevy_camera::visibility::Visibility',
      shortName: 'Visibility',
      fields: [{ name: '', kind: 'enum', options: ['Inherited', 'Hidden', 'Visible'] }],
      defaultValue: () => 'Inherited',
    };
    expect(flattenFields(visibility, 'Hidden')).toEqual([
      { label: 'value', binding: { component: visibility.typePath, path: '', kind: 'enum' }, value: 'Hidden', options: ['Inherited', 'Hidden', 'Visible'] },
    ]);

    const childOf: ComponentSchema = {
      typePath: 'bevy_ecs::hierarchy::ChildOf',
      shortName: 'ChildOf',
      fields: [{ name: '0', kind: 'entity' }],
      defaultValue: () => 0,
    };
    expect(flattenFields(childOf, 7)).toEqual([
      { label: 'value', binding: { component: childOf.typePath, path: '', kind: 'entity' }, value: 7, options: undefined },
    ]);
  });

  it('returns no rows for markers and opaque schemas', () => {
    const marker: ComponentSchema = { typePath: 'a::Tag', shortName: 'Tag', fields: 'marker', defaultValue: () => ({}) };
    const opaque: ComponentSchema = { typePath: 'a::Weird', shortName: 'Weird', fields: 'opaque', defaultValue: () => null };
    expect(flattenFields(marker, {})).toEqual([]);
    expect(flattenFields(opaque, { anything: 1 })).toEqual([]);
  });
});

describe('tupleWireShape', () => {
  it('converts a multi-field tuple struct object to the array Bevy expects on the wire', () => {
    const pair: ComponentSchema = {
      typePath: 'test::Pair',
      shortName: 'Pair',
      fields: [
        { name: '0', kind: 'string' },
        { name: '1', kind: 'f32' },
      ],
      defaultValue: () => ({ 0: '', 1: 0 }),
    };
    expect(tupleWireShape(pair, { 0: 'a', 1: 2 })).toEqual(['a', 2]);
  });

  it('passes single-field tuple structs (and other schemas) through unchanged', () => {
    const single: ComponentSchema = {
      typePath: 'test::Single',
      shortName: 'Single',
      fields: [{ name: '0', kind: 'f32' }],
      defaultValue: () => 0,
    };
    expect(tupleWireShape(single, 5)).toBe(5);

    const marker: ComponentSchema = { typePath: 'a::Tag', shortName: 'Tag', fields: 'marker', defaultValue: () => ({}) };
    expect(tupleWireShape(marker, {})).toEqual({});
  });
});
