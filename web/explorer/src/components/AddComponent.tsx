// AddComponent.tsx: registry-fed palette for adding a component to the selected
// entity. Ported from the PoC's addcomp overlay (search, hot item, Enter/Escape).
import { useEffect, useRef, useState } from 'preact/hooks';
import { Braces, Search } from 'lucide-preact';
import { Icon } from './Icon';
import { loadRegistry, type ComponentSchema } from '../lib/registry';
import { tupleWireShape } from '../lib/inspector';
import { world } from '../lib/brp';
import { toast } from '../lib/toasts';

async function insertDefault(entity: number, schema: ComponentSchema) {
  const value = tupleWireShape(schema, schema.defaultValue());
  await world.insertComponents(entity, { [schema.typePath]: value });
  toast('ok', `world.insert_components: ${schema.shortName}`);
}

export function AddComponent({
  entity,
  existing,
  onClose,
  onInserted,
}: {
  entity: number;
  existing: Set<string>;
  onClose: () => void;
  onInserted: () => void;
}) {
  const [registry, setRegistry] = useState<Map<string, ComponentSchema> | null>(null);
  const [query, setQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    loadRegistry()
      .then(setRegistry)
      .catch(() => {
        // Leave the palette empty (search will show "no matching components").
      });
  }, []);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const items = registry
    ? Array.from(registry.values())
        .filter((schema) => !existing.has(schema.typePath))
        .filter((schema) => (schema.shortName + schema.typePath).toLowerCase().includes(query.toLowerCase()))
    : [];

  async function pick(schema: ComponentSchema) {
    try {
      await insertDefault(entity, schema);
      onInserted();
    } catch (err) {
      toast('err', err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div
      class="overlay open"
      onClick={(ev) => {
        if (ev.target === ev.currentTarget) onClose();
      }}
    >
      <div class="palette">
        <div class="pal-input">
          <Icon of={Search} />
          <input
            ref={inputRef}
            placeholder="Add component"
            spellcheck={false}
            value={query}
            onInput={(ev) => setQuery((ev.target as HTMLInputElement).value)}
            onKeyDown={(ev) => {
              if (ev.key === 'Enter' && items[0]) void pick(items[0]);
              if (ev.key === 'Escape') onClose();
            }}
          />
        </div>
        <div class="pal-list">
          {items.length === 0 ? (
            <div class="pal-section">No matching registered components</div>
          ) : (
            items.map((schema, index) => (
              <button
                class={`pal-item${index === 0 ? ' hot' : ''}`}
                key={schema.typePath}
                onClick={() => void pick(schema)}
              >
                <Icon of={Braces} />
                <span>{schema.shortName}</span>
                <span class="hint" style="font-family:var(--font-mono)">
                  {schema.typePath}
                </span>
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
