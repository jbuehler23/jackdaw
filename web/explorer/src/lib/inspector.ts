// inspector.ts: pure helpers behind the schema-driven inspector. No BRP calls here;
// Inspector.tsx and FieldEditors.tsx own the network/DOM side, this module only
// shapes data: which rows a component's schema renders, immutable nested writes,
// and the drag-scrub arithmetic for number cells.
import type { ComponentSchema, FieldKind } from './registry';

export interface FieldBinding {
  component: string;
  path: string;
  kind: FieldKind;
}

export interface FieldRow {
  label: string;
  binding: FieldBinding;
  value: unknown;
  options?: string[];
}

function getByPath(value: unknown, path: string): unknown {
  if (path === '') return value;
  let cur = value;
  for (const part of path.split('.')) {
    if (cur === null || typeof cur !== 'object') return undefined;
    cur = (cur as Record<string, unknown>)[part];
  }
  return cur;
}

// Wire-shape rows for a component's current value, driven by its registry schema.
// A single-field tuple struct (registry unwraps it to a bare value) and a unit enum
// both render as one "value" row addressed by the empty path, since the whole
// component has to be replaced on write (mutate_components has no empty-path form).
export function flattenFields(schema: ComponentSchema, value: unknown): FieldRow[] {
  if (schema.fields === 'marker' || schema.fields === 'opaque') return [];
  const fields = schema.fields;
  if (fields.length === 1 && (fields[0].kind === 'enum' || fields[0].name === '0')) {
    const field = fields[0];
    return [
      {
        label: 'value',
        binding: { component: schema.typePath, path: '', kind: field.kind },
        value,
        options: field.options,
      },
    ];
  }
  return fields.map((field) => ({
    label: field.name,
    binding: { component: schema.typePath, path: field.name, kind: field.kind },
    value: getByPath(value, field.name),
    options: field.options,
  }));
}

export function scrubValue(start: number, dxPixels: number, step: number, shift: boolean): number {
  return start + dxPixels * step * (shift ? 10 : 1);
}

export function commitPath(value: unknown, path: string, next: unknown): unknown {
  if (path === '') return next;
  return setAtPath(value, path.split('.'), next);
}

function setAtPath(value: unknown, parts: string[], next: unknown): unknown {
  const [head, ...rest] = parts;
  if (Array.isArray(value)) {
    const copy = value.slice();
    copy[Number(head)] = rest.length ? setAtPath(copy[Number(head)], rest, next) : next;
    return copy;
  }
  const obj = (value && typeof value === 'object' ? value : {}) as Record<string, unknown>;
  return { ...obj, [head]: rest.length ? setAtPath(obj[head], rest, next) : next };
}

// Bevy's reflect wire format serializes a multi-field tuple struct as a JSON array,
// but registry.ts's defaultValue() for one (built from named struct-style fields)
// comes back as an object keyed by numeric strings ({0: .., 1: ..}). This converts
// that shape to the array Bevy expects before an insert_components call. Everything
// else (structs, unit enums, single-field tuple structs already unwrapped to a bare
// value) passes through unchanged.
export function tupleWireShape(schema: ComponentSchema, value: unknown): unknown {
  if (schema.fields === 'marker' || schema.fields === 'opaque') return value;
  const fields = schema.fields;
  if (fields.length <= 1) return value;
  const isTupleStruct = fields.every((field, index) => field.name === String(index));
  if (!isTupleStruct) return value;
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return value;
  const obj = value as Record<string, unknown>;
  return fields.map((field) => obj[field.name]);
}
