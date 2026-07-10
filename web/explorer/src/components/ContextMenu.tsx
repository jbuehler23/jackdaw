// ContextMenu.tsx: right-click menu for a tree row (spawn child, duplicate, despawn).
// Renders into document.body so it isn't clipped by the tree pane's scroll area.
import { createPortal } from 'preact/compat';
import { useEffect, useRef } from 'preact/hooks';
import { Copy, Plus, Trash2 } from 'lucide-preact';
import { Icon } from './Icon';
import { CHILD_OF } from '../lib/tree';
import { world } from '../lib/brp';
import { toast } from '../lib/toasts';

export interface ContextMenuTarget {
  entity: number;
  parent: number | null;
  x: number;
  y: number;
}

async function spawnChild(target: ContextMenuTarget) {
  const child = await world.spawnEntity({});
  await world.reparentEntities([child], target.entity);
  toast('ok', `world.spawn_entity: spawned ${child}`);
}

async function duplicate(target: ContextMenuTarget) {
  const componentNames = await world.listComponents(target.entity);
  const components = await world.getComponents(target.entity, componentNames);
  const withoutChildOf: Record<string, unknown> = { ...components };
  delete withoutChildOf[CHILD_OF];
  const copy = await world.spawnEntity(withoutChildOf);
  if (target.parent !== null) await world.reparentEntities([copy], target.parent);
  toast('ok', `world.spawn_entity: duplicated ${target.entity} as ${copy}`);
}

async function despawn(target: ContextMenuTarget) {
  await world.despawnEntity(target.entity);
  toast('ok', `world.despawn_entity: despawned ${target.entity}`);
}

export function ContextMenu({
  target,
  onClose,
  onAction,
}: {
  target: ContextMenuTarget;
  onClose: () => void;
  onAction: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const closeOnOutsideClick = () => onClose();
    document.addEventListener('click', closeOnOutsideClick);
    return () => document.removeEventListener('click', closeOnOutsideClick);
  }, [onClose]);

  async function run(action: (t: ContextMenuTarget) => Promise<void>) {
    try {
      await action(target);
      onAction();
    } catch (err) {
      toast('err', err instanceof Error ? err.message : String(err));
    }
    onClose();
  }

  const width = 190;
  const height = ref.current?.offsetHeight ?? 0;
  const left = Math.min(target.x, window.innerWidth - width);
  const top = Math.min(target.y, window.innerHeight - height - 6);

  return createPortal(
    <div
      class="ctx-menu open"
      ref={ref}
      style={`left:${left}px; top:${top}px`}
      onClick={(ev) => ev.stopPropagation()}
    >
      <button onClick={() => void run(spawnChild)}>
        <Icon of={Plus} />
        Spawn child
      </button>
      <button onClick={() => void run(duplicate)}>
        <Icon of={Copy} />
        Duplicate
      </button>
      <div class="ctx-sep" />
      <button class="danger" onClick={() => void run(despawn)}>
        <Icon of={Trash2} />
        Despawn
      </button>
    </div>,
    document.body,
  );
}
