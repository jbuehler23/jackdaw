// TreePanel.tsx: entity tree pane (filter box, spawn button, expand/collapse,
// selection, context menu). Ported from the PoC's renderTree/renderTreeNode.
import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import { ChevronRight, ListTree, Plus, Search } from 'lucide-preact';
import { Icon } from './Icon';
import { ContextMenu, type ContextMenuTarget } from './ContextMenu';
import { assembleTree, fetchTreeRows, type TreeNode } from '../lib/tree';
import { world } from '../lib/brp';
import { createPoll } from '../lib/poll';
import { selectedEntity } from '../lib/state';
import { entityLabel } from '../lib/format';
import { toast } from '../lib/toasts';

export const treePoll = createPoll(fetchTreeRows, 2000);

const filterText = signal('');
const expanded = signal<Set<number>>(new Set());
const ctxMenu = signal<ContextMenuTarget | null>(null);

function matchesFilter(node: TreeNode, filter: string): boolean {
  if (!filter) return true;
  const label = (node.name ?? 'Entity').toLowerCase();
  if (label.includes(filter)) return true;
  return node.children.some((child) => matchesFilter(child, filter));
}

function toggleExpanded(entity: number) {
  const next = new Set(expanded.value);
  if (next.has(entity)) next.delete(entity);
  else next.add(entity);
  expanded.value = next;
}

async function spawnRoot() {
  try {
    const entity = await world.spawnEntity({});
    selectedEntity.value = entity;
    toast('ok', `world.spawn_entity: spawned ${entity}`);
    await treePoll.refresh();
  } catch (err) {
    toast('err', err instanceof Error ? err.message : String(err));
  }
}

function TreeRow({ node, depth, filter }: { node: TreeNode; depth: number; filter: string }) {
  if (!matchesFilter(node, filter)) return null;
  const hasChildren = node.children.length > 0;
  const open = expanded.value.has(node.entity) || filter !== '';

  return (
    <>
      <div
        class={`tree-row${node.entity === selectedEntity.value ? ' selected' : ''}${node.name ? '' : ' unnamed'}`}
        style={`padding-left:${6 + depth * 14}px`}
        onClick={() => {
          selectedEntity.value = node.entity;
        }}
        onContextMenu={(ev) => {
          ev.preventDefault();
          ctxMenu.value = { entity: node.entity, parent: node.parent, x: ev.clientX, y: ev.clientY };
        }}
      >
        <button
          class={`twist${hasChildren ? (open ? ' open' : '') : ' leaf'}`}
          onClick={(ev) => {
            ev.stopPropagation();
            toggleExpanded(node.entity);
          }}
        >
          <Icon of={ChevronRight} />
        </button>
        <span class="kind-dot" style={`background:var(--dot-${node.kind})`} />
        <span class="ename">{node.name ?? 'Entity'}</span>
        <span class="eid">{entityLabel(node.entity)}</span>
      </div>
      {open && node.children.map((child) => <TreeRow key={child.entity} node={child} depth={depth + 1} filter={filter} />)}
    </>
  );
}

export function TreePanel() {
  useEffect(() => {
    treePoll.start();
    return () => treePoll.stop();
  }, []);

  const rows = treePoll.data.value;
  const roots = rows ? assembleTree(rows) : [];
  const filter = filterText.value.toLowerCase();

  return (
    <div class="pane" id="tree-pane" style="width:270px; flex:none">
      <div class="pane-header">
        <Icon of={ListTree} />
        Entities
      </div>
      <div class="tree-tools">
        <div class="search-box">
          <Icon of={Search} />
          <input
            placeholder="Filter entities"
            spellcheck={false}
            value={filterText.value}
            onInput={(ev) => {
              filterText.value = (ev.target as HTMLInputElement).value;
            }}
          />
        </div>
        <button class="tool-btn" title="Spawn entity" onClick={() => void spawnRoot()}>
          <Icon of={Plus} />
        </button>
      </div>
      <div class="pane-body">
        <div class="tree">
          {roots.map((node) => (
            <TreeRow key={node.entity} node={node} depth={0} filter={filter} />
          ))}
        </div>
      </div>
      {ctxMenu.value && (
        <ContextMenu
          target={ctxMenu.value}
          onClose={() => {
            ctxMenu.value = null;
          }}
          onAction={() => void treePoll.refresh()}
        />
      )}
    </div>
  );
}
