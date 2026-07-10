// Inspector.tsx: schema-driven property panel for the selected entity. Polls
// world.list_components + world.get_components while something is selected,
// renders one card per component from the registry's schema, and routes every
// field edit through mutate_components (or insert_components for whole-value
// fields: unit enums, single-field tuple structs) with a toast per write.
import { useEffect, useState } from 'preact/hooks';
import { signal } from '@preact/signals';
import { Braces, Plus, X } from 'lucide-preact';
import { Icon } from './Icon';
import { AddComponent } from './AddComponent';
import { BoolSwitch, ColorField, EntityLink, EnumDataField, EnumSelect, NumberCell, OpaqueJson, StringInput, VecRow } from './FieldEditors';
import { commitPath, flattenFields, type FieldRow } from '../lib/inspector';
import { loadRegistry, type ComponentSchema } from '../lib/registry';
import { jackdaw, world } from '../lib/brp';
import { createPoll } from '../lib/poll';
import { selectedEntity } from '../lib/state';
import { entityLabel, shortTypeName } from '../lib/format';
import { toast } from '../lib/toasts';
import { capabilities } from '../lib/connection';
import { treePoll } from './TreePanel';
import { assembleTree, type EntityKind, type TreeNode } from '../lib/tree';

interface InspectorData {
  entity: number;
  components: Record<string, unknown>;
}

async function fetchInspectorData(): Promise<InspectorData | null> {
  const entity = selectedEntity.value;
  if (entity === null) return null;
  const names = await world.listComponents(entity);
  const components = await world.getComponents(entity, names);
  return { entity, components };
}

export const inspectorPoll = createPoll(fetchInspectorData, 1000);

const registry = signal<Map<string, ComponentSchema>>(new Map());
let registryRequested = false;
function ensureRegistryLoaded() {
  if (registryRequested) return;
  registryRequested = true;
  loadRegistry()
    .then((loaded) => {
      registry.value = loaded;
    })
    .catch(() => {
      // Registry unavailable; components render as unregistered/read-only.
    });
}

function flattenTree(nodes: TreeNode[], out: Map<number, TreeNode>): Map<number, TreeNode> {
  for (const node of nodes) {
    out.set(node.entity, node);
    flattenTree(node.children, out);
  }
  return out;
}

// Walks the same tree the tree panel renders (world.query rows assembled by
// tree.ts) so the header's name/path/kind line always agrees with the tree.
function entityPathAndKind(entity: number): { name: string | null; path: string; kind: EntityKind } {
  const rows = treePoll.data.value;
  const fallback = { name: null, path: `/${entityLabel(entity)}`, kind: 'entity' as EntityKind };
  if (!rows) return fallback;
  const byEntity = flattenTree(assembleTree(rows), new Map());
  const node = byEntity.get(entity);
  if (!node) return fallback;
  const chain: string[] = [];
  let cur: TreeNode | undefined = node;
  while (cur) {
    chain.unshift(cur.name ?? entityLabel(cur.entity));
    cur = cur.parent !== null ? byEntity.get(cur.parent) : undefined;
  }
  return { name: node.name, path: `/${chain.join('/')}`, kind: node.kind };
}

async function commitField(entity: number, component: string, currentValue: unknown, path: string, next: unknown) {
  const shortName = registry.value.get(component)?.shortName ?? shortTypeName(component);
  try {
    if (path === '') {
      // Whole-value fields (unit enums, single-field tuple structs already
      // unwrapped to a bare value): mutate_components has no empty-path form.
      await world.insertComponents(entity, { [component]: next });
      toast('ok', `world.insert_components: ${shortName}`);
    } else {
      await world.mutateComponents(entity, component, path, next);
      toast('ok', `world.mutate_components: ${shortName}.${path}`);
    }
    const data = inspectorPoll.data.value;
    if (data && data.entity === entity) {
      const updated = commitPath(currentValue, path, next);
      inspectorPoll.data.value = { entity, components: { ...data.components, [component]: updated } };
    }
  } catch (err) {
    toast('err', err instanceof Error ? err.message : String(err));
  }
}

async function removeComponent(entity: number, component: string) {
  try {
    await world.removeComponents(entity, [component]);
    toast('ok', `world.remove_components: ${shortTypeName(component)}`);
    await inspectorPoll.refresh();
    await treePoll.refresh();
  } catch (err) {
    toast('err', err instanceof Error ? err.message : String(err));
  }
}

async function copyBsn(entity: number) {
  try {
    const { bsn } = await jackdaw.entityBsn(entity);
    await navigator.clipboard.writeText(bsn);
    toast('ok', 'jackdaw/entity_bsn: copied to clipboard');
  } catch (err) {
    toast('err', err instanceof Error ? err.message : String(err));
  }
}

const NUMERIC_COLS_KINDS = new Set(['vec2', 'vec3', 'vec4', 'quat', 'f32', 'color']);
const LABEL_TINT: Partial<Record<FieldRow['binding']['kind'], string>> = {
  f32: 't-num',
  vec2: 't-num',
  vec3: 't-num',
  vec4: 't-num',
  quat: 't-num',
  color: 't-num',
  bool: 't-bool',
  string: 't-str',
  enum: 't-enum',
  enumdata: 't-enum',
  entity: 't-entity',
};

function FieldEditor({ row, onCommit }: { row: FieldRow; onCommit: (path: string, value: unknown) => void }) {
  const { binding, value } = row;
  switch (binding.kind) {
    case 'vec2':
    case 'vec3':
    case 'vec4':
    case 'quat':
      return <VecRow binding={binding} value={value as Record<string, number> | number[]} onCommit={onCommit} />;
    case 'f32':
      return <NumberCell binding={binding} value={value as number} step={0.1} onCommit={onCommit} />;
    case 'bool':
      return <BoolSwitch binding={binding} value={value as boolean} onCommit={onCommit} />;
    case 'enum':
      return <EnumSelect binding={binding} value={value as string} options={row.options ?? []} onCommit={onCommit} />;
    case 'enumdata':
      return <EnumDataField binding={binding} value={value} variants={row.variants ?? []} onCommit={onCommit} />;
    case 'string':
      return <StringInput binding={binding} value={value as string} onCommit={onCommit} />;
    case 'entity':
      return <EntityLink value={value as number} />;
    case 'color':
      return <ColorField value={value} />;
    case 'json':
    default:
      return <OpaqueJson value={value} />;
  }
}

function ComponentCard({ entity, component, value }: { entity: number; component: string; value: unknown }) {
  const schema = registry.value.get(component);
  const shortName = schema?.shortName ?? shortTypeName(component);

  let body;
  if (!schema) {
    body = <div class="marker-note">No schema in registry; read-only.</div>;
  } else if (schema.fields === 'marker') {
    body = <div class="marker-note">Marker component (no fields)</div>;
  } else if (schema.fields === 'opaque') {
    body = <OpaqueJson value={value} />;
  } else {
    const rows = flattenFields(schema, value);
    body = (
      <>
        {rows.map((row) => {
          const cols = NUMERIC_COLS_KINDS.has(row.binding.kind);
          const tint = LABEL_TINT[row.binding.kind] ?? '';
          return (
            <div class="field-row" key={row.binding.path || row.label}>
              <span class={`field-label ${tint}`} title={row.label}>
                {row.label}
              </span>
              <div class={`field-input${cols ? ' cols' : ''}`}>
                <FieldEditor row={row} onCommit={(path, next) => void commitField(entity, component, value, path, next)} />
              </div>
            </div>
          );
        })}
      </>
    );
  }

  return (
    <div class="comp-card">
      <header>
        <span>{shortName}</span>
        <span class="full">{component}</span>
        <button class="x" title="Remove component" onClick={() => void removeComponent(entity, component)}>
          <Icon of={X} />
        </button>
      </header>
      <div class="comp-fields">{body}</div>
    </div>
  );
}

export function Inspector() {
  const [addOpen, setAddOpen] = useState(false);

  useEffect(() => {
    ensureRegistryLoaded();
    inspectorPoll.start();
    return () => inspectorPoll.stop();
  }, []);

  const entity = selectedEntity.value;
  useEffect(() => {
    void inspectorPoll.refresh();
  }, [entity]);

  if (entity === null) {
    return (
      <div class="pane" style="flex:1">
        <div class="insp-empty">
          Select an entity to inspect its components.
          <br />
          <br />
          Values update live from the running app and edits are written back over BRP.
        </div>
      </div>
    );
  }

  const data = inspectorPoll.data.value;
  const loaded = data && data.entity === entity ? data : null;
  const { name, path, kind } = entityPathAndKind(entity);
  const canBsn = capabilities.value.has('jackdaw/entity_bsn');
  const existing = new Set(Object.keys(loaded?.components ?? {}));

  return (
    <div class="pane" style="flex:1">
      <div class="insp-head">
        <div class="title">
          <span class="kind-dot" style={`background:var(--dot-${kind})`} />
          <span>{name ?? 'Entity'}</span>
          {canBsn && (
            <button class="head-act" title="Copy this entity as BSN" onClick={() => void copyBsn(entity)}>
              <Icon of={Braces} />
              BSN
            </button>
          )}
        </div>
        <div class="meta">
          {entityLabel(entity)} · {path}
        </div>
      </div>
      {loaded ? (
        <div class="pane-body">
          <div class="comp-cards">
            {Object.keys(loaded.components).map((component) => (
              <ComponentCard key={component} entity={entity} component={component} value={loaded.components[component]} />
            ))}
          </div>
          <button class="add-comp" onClick={() => setAddOpen(true)}>
            <Icon of={Plus} />
            Add component
          </button>
        </div>
      ) : (
        <div class="pane-body">
          <div class="marker-note">Loading components</div>
        </div>
      )}
      {addOpen && (
        <AddComponent
          entity={entity}
          existing={existing}
          onClose={() => setAddOpen(false)}
          onInserted={() => {
            setAddOpen(false);
            void inspectorPoll.refresh();
            void treePoll.refresh();
          }}
        />
      )}
    </div>
  );
}
