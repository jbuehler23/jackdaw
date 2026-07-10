// ArchetypesTab.tsx: live archetype table, ported from the PoC's
// arch-scroll/#arch-table markup (.scratch/web-explorer/jackdaw-explorer-poc.html).
// Fed by `jackdaw/archetypes` (already sorted by entity_count desc); the
// "query" button per row seeds the Queries page's chip builder.
import { useEffect, useState } from 'preact/hooks';
import { Layers, Search, Zap } from 'lucide-preact';
import { Icon } from './Icon';
import { jackdaw, type ArchetypeInfo } from '../lib/brp';
import { createPoll } from '../lib/poll';
import { capabilities } from '../lib/connection';
import { page } from '../lib/state';
import { shortTypeName } from '../lib/format';
import { loadRegistry, type ComponentSchema } from '../lib/registry';
import { seedQuery } from './QueriesPage';

const archetypesPoll = createPoll(jackdaw.archetypes, 2000);

/** Splits an archetype's components into up to 3 non-marker components to
 * fetch and up to 2 marker components to filter with, per the PoC's
 * archetype-to-query seeding. Components missing from the registry (schema
 * not loaded, or not reflected) are treated as non-marker. */
export function seedFromArchetype(
  components: string[],
  registry: Map<string, ComponentSchema> | null,
): { fetch: string[]; withList: string[] } {
  const isMarker = (c: string) => registry?.get(c)?.fields === 'marker';
  const fetch = components.filter((c) => !isMarker(c)).slice(0, 3);
  const withList = components.filter((c) => isMarker(c)).slice(0, 2);
  return { fetch, withList };
}

export function ArchetypesTab() {
  const [registry, setRegistry] = useState<Map<string, ComponentSchema> | null>(null);
  const canArchetypes = capabilities.value.has('jackdaw/archetypes');

  useEffect(() => {
    loadRegistry()
      .then(setRegistry)
      .catch(() => {
        // Falls back to treating every component as non-marker for seeding.
      });
  }, []);

  useEffect(() => {
    if (!canArchetypes) return;
    archetypesPoll.start();
    return () => archetypesPoll.stop();
  }, [canArchetypes]);

  if (!canArchetypes) {
    return (
      <div class="pane" style="flex:1">
        <div class="pane-header">
          <Icon of={Layers} />
          Archetypes
        </div>
        <div class="stats-wrap">
          <div class="stats-note">
            <Icon of={Zap} />
            <span>
              Served by the <code>jackdaw/archetypes</code> method from <code>JackdawRemotePlugin</code>. Apps
              running plain BRP still get the tree, inspector, and queries; this page shows an upgrade hint instead.
            </span>
          </div>
        </div>
      </div>
    );
  }

  const archetypes: ArchetypeInfo[] = archetypesPoll.data.value?.archetypes ?? [];
  const maxCount = Math.max(1, ...archetypes.map((a) => a.entity_count));
  const totalEntities = archetypes.reduce((sum, a) => sum + a.entity_count, 0);

  function runQueryFor(components: string[]) {
    const { fetch, withList } = seedFromArchetype(components, registry);
    seedQuery(fetch, withList);
    page.value = 'queries';
  }

  return (
    <div class="arch-scroll">
      <div class="arch-meta">
        {archetypes.length} archetypes · {totalEntities} entities
      </div>
      <table class="results">
        <thead>
          <tr>
            <th>Components</th>
            <th>Entities</th>
            <th></th>
            <th>Bytes/entity</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {archetypes.map((arch, i) => (
            <tr key={i}>
              <td style="white-space:normal">
                {arch.components.map((c) => (
                  <span class="comp-chip" title={c} key={c}>
                    {registry?.get(c)?.shortName ?? shortTypeName(c)}
                  </span>
                ))}
              </td>
              <td>{arch.entity_count}</td>
              <td>
                <span class="count-bar">
                  <i style={`width:${(arch.entity_count / maxCount) * 100}%`} />
                </span>
              </td>
              <td>{arch.bytes_per_entity}</td>
              <td>
                <button class="entity-link" onClick={() => runQueryFor(arch.components)}>
                  <Icon of={Search} />
                  query
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
