// registry.ts: registry.schema -> editable field definitions.
//
// Raw shape verified against bevy_remote's JsonSchemaBevyType
// (crates/bevy_remote/src/schemas/json_schema.rs): the struct derives
// #[serde(rename_all = "camelCase")], so field names on the wire are
// shortPath, kind, properties, required, oneOf, prefixItems, type. `kind`
// is the SchemaKind enum serialized as its bare variant name (Struct,
// TupleStruct, Enum, Value, ...). Struct/TupleStruct field entries and
// tuple prefixItems entries are `{ type: { $ref: "#/$defs/<type_path>" } }`.
// Unit-only enums serialize `oneOf` as a plain array of variant name
// strings. registry.schema's top-level result is a map of type path to
// this schema object, with no extra wrapping. All of this matches the
// brief's assumed shape; no field renames or restructuring were needed.
import { brpCall } from './brp';
import { shortTypeName } from './format';

export type FieldKind =
  | 'f32' | 'bool' | 'string' | 'enum' | 'enumdata'
  | 'vec2' | 'vec3' | 'vec4' | 'quat' | 'color' | 'entity' | 'json';

// glam's vec types (Vec2/3/4, IVec*, UVec*, DVec*) all use the same #[reflect(Serialize,
// Deserialize)] treatment: the wire value is whatever their own serde impl produces (a
// bare JSON array of components), not a structural {x,y,...} object built from reflected
// fields. See bevy_reflect's crates/bevy_reflect/src/impls/glam.rs (impl_reflect! with
// #[type_path = "glam"], so the $ref path is "glam::<Type>") and glam's
// src/features/impl_serde.rs (serialize_tuple_struct of the axis values -> JSON array).

// A data-carrying enum whose variants are either unit (no payload) or a tuple
// variant with a single numeric field, e.g. bevy_text's LineHeight::Px(f32) /
// RelativeToFont(f32) or LetterSpacing::Px(f32) / Rem(f32).
export interface EnumDataVariant { name: string; payload: 'f32' | 'none' }

export interface FieldDef { name: string; kind: FieldKind; options?: string[]; variants?: EnumDataVariant[] }

export interface ComponentSchema {
  typePath: string;
  shortName: string;
  fields: FieldDef[] | 'marker' | 'opaque';
  defaultValue(): unknown;
}

interface RawSchema {
  shortPath?: string;
  kind?: string;
  properties?: Record<string, { type?: { $ref?: string } }>;
  oneOf?: unknown[];
  prefixItems?: { type?: { $ref?: string } }[];
}

// Enum variant entries as they appear in a mixed (data-carrying) enum's oneOf:
// unit variants have no "kind"/"type" (just typePath/shortPath); tuple variants
// are {type:"array", kind:"Tuple", shortPath, prefixItems, items:false}; struct
// variants are {type:"object", kind:"Struct", shortPath, properties, ...}. See
// bevy_remote's json_schema.rs SchemaJsonReference/VariantInfo mapping.
interface RawEnumVariant {
  shortPath?: string;
  kind?: string;
  prefixItems?: { type?: { $ref?: string } }[];
}

const REF_KINDS: Record<string, FieldKind> = {
  'glam::Vec2': 'vec2',
  'glam::Vec3': 'vec3',
  'glam::Vec3A': 'vec3',
  'glam::Vec4': 'vec4',
  'glam::Quat': 'quat',
  'glam::UVec2': 'vec2',
  'glam::UVec3': 'vec3',
  'glam::UVec4': 'vec4',
  'glam::IVec2': 'vec2',
  'glam::IVec3': 'vec3',
  'glam::IVec4': 'vec4',
  'glam::DVec2': 'vec2',
  'glam::DVec3': 'vec3',
  'glam::DVec4': 'vec4',
  f32: 'f32', f64: 'f32',
  u8: 'f32', u16: 'f32', u32: 'f32', u64: 'f32', usize: 'f32',
  i8: 'f32', i16: 'f32', i32: 'f32', i64: 'f32', isize: 'f32',
  bool: 'bool',
  'alloc::string::String': 'string',
  'alloc::borrow::Cow<str>': 'string',
  'bevy_color::color::Color': 'color',
  'bevy_ecs::entity::Entity': 'entity',
};

// Resolves a struct/tuple-struct field's $ref one level deep: primitives and
// glam/color/entity types map directly; a $ref to another registered type is
// only inlined when that type is a unit enum (kind Enum, all-string oneOf),
// giving the field 'enum' + its options. Anything else (Handles, data-carrying
// enums, nested structs) stays 'json' so it renders read-only instead of
// collapsing the whole component to opaque.
function resolveRef(ref: string | undefined, defs: Record<string, RawSchema> | undefined): { kind: FieldKind; options?: string[] } {
  if (!ref) return { kind: 'json' };
  const path = ref.replace('#/$defs/', '');
  const primitive = REF_KINDS[path];
  if (primitive) return { kind: primitive };
  const def = defs?.[path];
  if (def?.kind === 'Enum' && Array.isArray(def.oneOf) && def.oneOf.every((v) => typeof v === 'string')) {
    return { kind: 'enum', options: def.oneOf as string[] };
  }
  return { kind: 'json' };
}

// One extra resolution level for a struct field whose own $ref points to another
// registered struct (e.g. bevy_ui/bevy_sprite's BorderRect nesting two glam Vec2s
// as min_inset/max_inset). Inlines that struct's fields as prefixed sub-rows only
// when EVERY one of its fields resolves (through the same primitive/glam/enum
// mapping above, not another nested struct) to something other than 'json'.
// Anything left over bails to null so the caller keeps the parent field a single
// opaque 'json' row instead of partially inlining it.
function resolveNestedStruct(
  ref: string | undefined,
  defs: Record<string, RawSchema> | undefined,
  parentName: string,
): FieldDef[] | null {
  if (!ref || !defs) return null;
  const path = ref.replace('#/$defs/', '');
  const def = defs[path];
  if (!def || (def.kind !== 'Struct' && def.kind !== 'TupleStruct')) return null;
  const props = def.kind === 'Struct' ? def.properties ?? {} : tupleProps(def);
  const names = Object.keys(props);
  if (names.length === 0) return null;
  const fields: FieldDef[] = [];
  for (const name of names) {
    const resolved = resolveRef(props[name].type?.$ref, defs);
    if (resolved.kind === 'json') return null;
    fields.push({ name: `${parentName}.${name}`, kind: resolved.kind, options: resolved.options });
  }
  return fields;
}

const FIELD_DEFAULTS: Record<FieldKind, () => unknown> = {
  f32: () => 0,
  bool: () => false,
  string: () => '',
  enum: () => '',
  enumdata: () => null,
  vec2: () => ({ x: 0, y: 0 }),
  vec3: () => ({ x: 0, y: 0, z: 0 }),
  vec4: () => ({ x: 0, y: 0, z: 0, w: 0 }),
  quat: () => ({ x: 0, y: 0, z: 0, w: 1 }),
  color: () => ({ Srgba: { red: 1, green: 1, blue: 1, alpha: 1 } }),
  entity: () => 0,
  json: () => null,
};

// A mixed enum's oneOf entries are eligible for 'enumdata' only when every
// variant is either unit (no payload) or a single-field tuple variant whose
// field is a plain number. Anything else (struct variants, multi-field
// tuples, non-numeric payloads) bails out to null so the caller keeps the
// enum opaque rather than mis-editing it.
function dataEnumVariants(oneOf: unknown[]): EnumDataVariant[] | null {
  const variants: EnumDataVariant[] = [];
  for (const raw of oneOf) {
    if (typeof raw !== 'object' || raw === null) return null;
    const variant = raw as RawEnumVariant;
    if (!variant.shortPath) return null;
    if (variant.kind === undefined) {
      variants.push({ name: variant.shortPath, payload: 'none' });
      continue;
    }
    if (variant.kind === 'Tuple' && Array.isArray(variant.prefixItems) && variant.prefixItems.length === 1) {
      const ref = variant.prefixItems[0]?.type?.$ref;
      const path = ref?.replace('#/$defs/', '');
      if (!path || REF_KINDS[path] !== 'f32') return null;
      variants.push({ name: variant.shortPath, payload: 'f32' });
      continue;
    }
    return null;
  }
  return variants.length > 0 ? variants : null;
}

function enumDataDefault(variant: EnumDataVariant): unknown {
  return variant.payload === 'none' ? variant.name : { [variant.name]: 0 };
}

// FIELD_DEFAULTS doesn't know a struct field's resolved enum options (an enum
// field default is its first option, not the bare '' placeholder used when an
// enum is the component's own top-level value with no field-local options).
function defaultForField(field: FieldDef): unknown {
  if (field.kind === 'enum' && field.options) return field.options[0] ?? '';
  return FIELD_DEFAULTS[field.kind]();
}

export function schemaToComponentSchema(
  typePath: string,
  raw: RawSchema,
  defs?: Record<string, RawSchema>,
): ComponentSchema {
  const shortName = raw.shortPath ?? shortTypeName(typePath);

  if (raw.kind === 'Enum') {
    if (Array.isArray(raw.oneOf) && raw.oneOf.every((v) => typeof v === 'string')) {
      const options = raw.oneOf as string[];
      return {
        typePath, shortName,
        fields: [{ name: '', kind: 'enum', options }],
        defaultValue: () => options[0] ?? '',
      };
    }
    const variants = Array.isArray(raw.oneOf) ? dataEnumVariants(raw.oneOf) : null;
    if (variants) {
      return {
        typePath, shortName,
        fields: [{ name: '', kind: 'enumdata', variants }],
        defaultValue: () => enumDataDefault(variants[0]),
      };
    }
    return { typePath, shortName, fields: 'opaque', defaultValue: () => null };
  }

  if (raw.kind === 'Struct' || raw.kind === 'TupleStruct') {
    const props = raw.kind === 'Struct' ? raw.properties ?? {} : tupleProps(raw);
    const names = Object.keys(props);
    if (names.length === 0) {
      return { typePath, shortName, fields: 'marker', defaultValue: () => ({}) };
    }
    const fields: FieldDef[] = names.flatMap((name) => {
      const resolved = resolveRef(props[name].type?.$ref, defs);
      if (resolved.kind === 'json') {
        const nested = resolveNestedStruct(props[name].type?.$ref, defs, name);
        if (nested) return nested;
      }
      return [{ name, kind: resolved.kind, options: resolved.options }];
    });
    if (fields.every((f) => f.kind === 'json')) {
      return { typePath, shortName, fields: 'opaque', defaultValue: () => null };
    }
    return {
      typePath, shortName, fields,
      defaultValue: () => {
        if (raw.kind === 'TupleStruct' && fields.length === 1) return defaultForField(fields[0]);
        const out: Record<string, unknown> = {};
        for (const f of fields) {
          const [head, ...rest] = f.name.split('.');
          if (rest.length === 0) {
            out[head] = defaultForField(f);
          } else {
            const parent = (out[head] as Record<string, unknown> | undefined) ?? {};
            parent[rest.join('.')] = defaultForField(f);
            out[head] = parent;
          }
        }
        return out;
      },
    };
  }

  return { typePath, shortName, fields: 'opaque', defaultValue: () => null };
}

function tupleProps(raw: RawSchema): Record<string, { type?: { $ref?: string } }> {
  const out: Record<string, { type?: { $ref?: string } }> = {};
  (raw.prefixItems ?? []).forEach((item, i) => {
    out[String(i)] = item;
  });
  return out;
}

let cached: Map<string, ComponentSchema> | null = null;

export async function loadRegistry(): Promise<Map<string, ComponentSchema>> {
  if (cached) return cached;
  const raw = await brpCall<Record<string, RawSchema>>('registry.schema');
  cached = new Map(Object.entries(raw).map(([path, entry]) => [path, schemaToComponentSchema(path, entry, raw)]));
  return cached;
}
