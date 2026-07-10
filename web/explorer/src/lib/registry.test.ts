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

  it('resolves a struct field $ref to a registered unit enum as an enum field with options', () => {
    const defs = {
      'bevy_text::text::Justify': { shortPath: 'Justify', kind: 'Enum', oneOf: ['Left', 'Center', 'Right'] },
      'bevy_text::text::LineBreak': { shortPath: 'LineBreak', kind: 'Enum', oneOf: ['WordBoundary', 'AnyCharacter'] },
    };
    const cs = schemaToComponentSchema(
      'bevy_text::text::TextLayout',
      {
        shortPath: 'TextLayout',
        kind: 'Struct',
        properties: {
          justify: { type: { $ref: '#/$defs/bevy_text::text::Justify' } },
          linebreak: { type: { $ref: '#/$defs/bevy_text::text::LineBreak' } },
        },
      },
      defs,
    );
    expect(cs.fields).toEqual([
      { name: 'justify', kind: 'enum', options: ['Left', 'Center', 'Right'] },
      { name: 'linebreak', kind: 'enum', options: ['WordBoundary', 'AnyCharacter'] },
    ]);
    expect(cs.defaultValue()).toEqual({ justify: 'Left', linebreak: 'WordBoundary' });
  });

  it('keeps a mixed struct mappable instead of collapsing it to opaque', () => {
    const cs = schemaToComponentSchema('bevy_text::text::TextFont', {
      shortPath: 'TextFont',
      kind: 'Struct',
      properties: {
        font: { type: { $ref: '#/$defs/bevy_asset::handle::Handle<bevy_text::font::Font>' } },
        font_size: { type: { $ref: '#/$defs/f32' } },
      },
    });
    expect(cs.fields).not.toBe('opaque');
    expect(cs.fields).toEqual([
      { name: 'font', kind: 'json' },
      { name: 'font_size', kind: 'f32' },
    ]);
    expect(cs.defaultValue()).toEqual({ font: null, font_size: 0 });
  });

  it('still collapses an all-json struct to opaque', () => {
    const cs = schemaToComponentSchema('bevy_transform::components::global_transform::GlobalTransform', {
      shortPath: 'GlobalTransform',
      kind: 'Struct',
      properties: {
        matrix: { type: { $ref: '#/$defs/glam::Affine3A' } },
      },
    });
    expect(cs.fields).toBe('opaque');
  });

  it('maps a data-carrying enum (Px/RelativeToFont-style tuple variants) to enumdata', () => {
    const cs = schemaToComponentSchema('bevy_text::text::LineHeight', {
      shortPath: 'LineHeight',
      kind: 'Enum',
      oneOf: [
        { type: 'array', kind: 'Tuple', typePath: 'bevy_text::text::LineHeight::Px', shortPath: 'Px', prefixItems: [{ type: { $ref: '#/$defs/f32' } }], items: false },
        {
          type: 'array', kind: 'Tuple', typePath: 'bevy_text::text::LineHeight::RelativeToFont', shortPath: 'RelativeToFont',
          prefixItems: [{ type: { $ref: '#/$defs/f32' } }], items: false,
        },
      ],
    });
    expect(cs.fields).toEqual([
      {
        name: '', kind: 'enumdata',
        variants: [
          { name: 'Px', payload: 'f32' },
          { name: 'RelativeToFont', payload: 'f32' },
        ],
      },
    ]);
    expect(cs.defaultValue()).toEqual({ Px: 0 });
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
