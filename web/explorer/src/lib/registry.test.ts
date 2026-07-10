import { describe, expect, it } from 'vitest';
import { schemaToComponentSchema } from './registry';

const VEC3_REF = { type: { $ref: '#/$defs/glam::Vec3' } };

describe('schemaToComponentSchema', () => {
  it('maps a struct with vec3/quat/f32 fields', () => {
    const cs = schemaToComponentSchema('bevy_transform::components::transform::Transform', {
      shortPath: 'Transform',
      kind: 'Struct',
      properties: {
        translation: VEC3_REF,
        rotation: { type: { $ref: '#/$defs/glam::Quat' } },
        scale: VEC3_REF,
      },
    });
    expect(cs.fields).toEqual([
      { name: 'translation', kind: 'vec3' },
      { name: 'rotation', kind: 'quat' },
      { name: 'scale', kind: 'vec3' },
    ]);
    const dv = cs.defaultValue() as Record<string, unknown>;
    expect(dv.translation).toEqual({ x: 0, y: 0, z: 0 });
    expect(dv.rotation).toEqual({ x: 0, y: 0, z: 0, w: 1 });
  });

  it('maps unit enums to enum fields with options', () => {
    const cs = schemaToComponentSchema('bevy_camera::visibility::Visibility', {
      shortPath: 'Visibility',
      kind: 'Enum',
      oneOf: ['Inherited', 'Hidden', 'Visible'],
    });
    expect(cs.fields).toEqual([{ name: '', kind: 'enum', options: ['Inherited', 'Hidden', 'Visible'] }]);
    expect(cs.defaultValue()).toBe('Inherited');
  });

  it('marks empty structs as markers and unknown shapes as opaque', () => {
    expect(schemaToComponentSchema('a::Tag', { shortPath: 'Tag', kind: 'Struct', properties: {} }).fields).toBe('marker');
    expect(
      schemaToComponentSchema('a::Weird', { shortPath: 'Weird', kind: 'Value' }).fields,
    ).toBe('opaque');
  });

  it('maps primitive numbers, bools, strings', () => {
    const cs = schemaToComponentSchema('game::Health', {
      shortPath: 'Health',
      kind: 'Struct',
      properties: {
        current: { type: { $ref: '#/$defs/f32' } },
        alive: { type: { $ref: '#/$defs/bool' } },
        label: { type: { $ref: '#/$defs/alloc::string::String' } },
      },
    });
    expect(cs.fields).toEqual([
      { name: 'current', kind: 'f32' },
      { name: 'alive', kind: 'bool' },
      { name: 'label', kind: 'string' },
    ]);
  });
});
