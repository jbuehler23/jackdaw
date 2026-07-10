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
  | 'f32' | 'bool' | 'string' | 'enum'
  | 'vec2' | 'vec3' | 'vec4' | 'quat' | 'color' | 'entity' | 'json';

export interface FieldDef { name: string; kind: FieldKind; options?: string[] }

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

const REF_KINDS: Record<string, FieldKind> = {
  'glam::Vec2': 'vec2',
  'glam::Vec3': 'vec3',
  'glam::Vec3A': 'vec3',
  'glam::Vec4': 'vec4',
  'glam::Quat': 'quat',
  f32: 'f32', f64: 'f32',
  u8: 'f32', u16: 'f32', u32: 'f32', u64: 'f32', usize: 'f32',
  i8: 'f32', i16: 'f32', i32: 'f32', i64: 'f32', isize: 'f32',
  bool: 'bool',
  'alloc::string::String': 'string',
  'alloc::borrow::Cow<str>': 'string',
  'bevy_color::color::Color': 'color',
  'bevy_ecs::entity::Entity': 'entity',
};

function refToKind(ref: string | undefined): FieldKind {
  if (!ref) return 'json';
  const path = ref.replace('#/$defs/', '');
  return REF_KINDS[path] ?? 'json';
}

const FIELD_DEFAULTS: Record<FieldKind, () => unknown> = {
  f32: () => 0,
  bool: () => false,
  string: () => '',
  enum: () => '',
  vec2: () => ({ x: 0, y: 0 }),
  vec3: () => ({ x: 0, y: 0, z: 0 }),
  vec4: () => ({ x: 0, y: 0, z: 0, w: 0 }),
  quat: () => ({ x: 0, y: 0, z: 0, w: 1 }),
  color: () => ({ Srgba: { red: 1, green: 1, blue: 1, alpha: 1 } }),
  entity: () => 0,
  json: () => null,
};

export function schemaToComponentSchema(typePath: string, raw: RawSchema): ComponentSchema {
  const shortName = raw.shortPath ?? shortTypeName(typePath);

  if (raw.kind === 'Enum' && Array.isArray(raw.oneOf) && raw.oneOf.every((v) => typeof v === 'string')) {
    const options = raw.oneOf as string[];
    return {
      typePath, shortName,
      fields: [{ name: '', kind: 'enum', options }],
      defaultValue: () => options[0] ?? '',
    };
  }

  if (raw.kind === 'Struct' || raw.kind === 'TupleStruct') {
    const props = raw.kind === 'Struct' ? raw.properties ?? {} : tupleProps(raw);
    const names = Object.keys(props);
    if (names.length === 0) {
      return { typePath, shortName, fields: 'marker', defaultValue: () => ({}) };
    }
    const fields: FieldDef[] = names.map((name) => ({ name, kind: refToKind(props[name].type?.$ref) }));
    if (fields.some((f) => f.kind === 'json')) {
      return { typePath, shortName, fields: 'opaque', defaultValue: () => null };
    }
    return {
      typePath, shortName, fields,
      defaultValue: () => {
        if (raw.kind === 'TupleStruct' && fields.length === 1) return FIELD_DEFAULTS[fields[0].kind]();
        return Object.fromEntries(fields.map((f) => [f.name, FIELD_DEFAULTS[f.kind]()]));
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
  cached = new Map(Object.entries(raw).map(([path, entry]) => [path, schemaToComponentSchema(path, entry)]));
  return cached;
}
