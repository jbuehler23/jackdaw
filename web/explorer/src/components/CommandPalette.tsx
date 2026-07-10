// CommandPalette.tsx: Ctrl+K overlay with substring-filtered entity jumps and
// shell actions. Ported from the PoC's paletteItems/renderPalette.
import { signal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { Box, Boxes, Braces, ChartLine, Network, Pause, Play, Search, SearchCode } from 'lucide-preact';
import type { LucideIcon } from 'lucide-preact';
import { Icon } from './Icon';
import { treePoll } from '../lib/treeData';
import { assembleTree, type TreeNode } from '../lib/tree';
import { page, pollingPaused, selectedEntity, simPaused, type Page } from '../lib/state';
import { toggleSim, togglePolling } from '../lib/commands';
import { entityLabel } from '../lib/format';

export const paletteOpen = signal(false);
const query = signal('');
const hot = signal(0);

interface PaletteItem {
  label: string;
  hint: string;
  icon: LucideIcon | null;
  dot: string | null;
  run: () => void;
}

const KIND_DOT: Record<TreeNode['kind'], string> = {
  camera: 'var(--dot-camera)',
  light: 'var(--dot-light)',
  mesh: 'var(--dot-mesh)',
  prefab: 'var(--dot-prefab)',
  entity: 'var(--dot-entity)',
};

function flattenTree(nodes: TreeNode[]): TreeNode[] {
  const out: TreeNode[] = [];
  for (const node of nodes) {
    out.push(node);
    out.push(...flattenTree(node.children));
  }
  return out;
}

function switchPage(target: Page) {
  page.value = target;
}

function selectEntity(entity: number) {
  selectedEntity.value = entity;
  page.value = 'entities';
}

function paletteItems(query: string): PaletteItem[] {
  const q = query.toLowerCase();
  const actions: PaletteItem[] = [
    { label: 'Go to Entities', hint: 'page', icon: Boxes, dot: null, run: () => switchPage('entities') },
    { label: 'Go to Queries', hint: 'page', icon: SearchCode, dot: null, run: () => switchPage('queries') },
    { label: 'Go to Stats', hint: 'page', icon: ChartLine, dot: null, run: () => switchPage('stats') },
    { label: 'Go to BSN', hint: 'page', icon: Braces, dot: null, run: () => switchPage('bsn') },
    { label: 'Go to Viewport', hint: 'page', icon: Box, dot: null, run: () => switchPage('viewport') },
    { label: 'Go to ECS internals', hint: 'page', icon: Network, dot: null, run: () => switchPage('ecs') },
    {
      label: pollingPaused.value ? 'Resume polling' : 'Pause polling',
      hint: 'connection',
      icon: pollingPaused.value ? Play : Pause,
      dot: null,
      run: togglePolling,
    },
    {
      label: simPaused.value ? 'Resume game simulation' : 'Pause game simulation',
      hint: 'jackdaw/playback',
      icon: simPaused.value ? Play : Pause,
      dot: null,
      run: () => void toggleSim(),
    },
  ].filter((a) => a.label.toLowerCase().includes(q));

  const rows = treePoll.data.value;
  const entities: PaletteItem[] = (rows ? flattenTree(assembleTree(rows)) : [])
    .filter((node) => (node.name ?? 'Entity').toLowerCase().includes(q))
    .slice(0, 9)
    .map((node) => ({
      label: node.name ?? 'Entity',
      hint: entityLabel(node.entity),
      icon: null,
      dot: KIND_DOT[node.kind],
      run: () => selectEntity(node.entity),
    }));

  return [...entities, ...actions];
}

export function CommandPalette() {
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (paletteOpen.value) {
      query.value = '';
      hot.value = 0;
      inputRef.current?.focus();
    }
  }, [paletteOpen.value]);

  if (!paletteOpen.value) return null;

  const items = paletteItems(query.value);
  const hotIndex = Math.min(hot.value, Math.max(0, items.length - 1));

  function run(item: PaletteItem) {
    item.run();
    paletteOpen.value = false;
  }

  return (
    <div
      class="overlay open"
      onClick={(ev) => {
        if (ev.target === ev.currentTarget) paletteOpen.value = false;
      }}
    >
      <div class="palette">
        <div class="pal-input">
          <Icon of={Search} />
          <input
            ref={inputRef}
            placeholder="Jump to entity, run a command"
            spellcheck={false}
            value={query.value}
            onInput={(ev) => {
              query.value = (ev.target as HTMLInputElement).value;
              hot.value = 0;
            }}
            onKeyDown={(ev) => {
              if (ev.key === 'ArrowDown') {
                ev.preventDefault();
                hot.value = Math.min(hot.value + 1, items.length - 1);
              } else if (ev.key === 'ArrowUp') {
                ev.preventDefault();
                hot.value = Math.max(hot.value - 1, 0);
              } else if (ev.key === 'Enter') {
                const item = items[hotIndex];
                if (item) run(item);
              } else if (ev.key === 'Escape') {
                paletteOpen.value = false;
              }
            }}
          />
        </div>
        <div class="pal-list">
          {items.length === 0 && <div class="pal-section">No matches</div>}
          {items.map((item, i) => (
            <button key={`${item.label}-${item.hint}`} class={`pal-item${i === hotIndex ? ' hot' : ''}`} onClick={() => run(item)}>
              {item.dot ? <span class="kind-dot" style={`background:${item.dot}`} /> : item.icon && <Icon of={item.icon} />}
              <span>{item.label}</span>
              <span class="hint">{item.hint}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
