// EcsPage.tsx: ECS internals page shell. Three doc-tabs (Relationships,
// Schedule, Archetypes) swapped by a local signal; Schedule and Archetypes are
// stubs here (Task 7 replaces them with the real views).
import { signal } from '@preact/signals';
import { GitFork, Layers, Workflow } from 'lucide-preact';
import { Icon } from './Icon';
import { RelationshipsTab } from './RelationshipsTab';

type EcsTab = 'rel' | 'sched' | 'arch';

const ecsTab = signal<EcsTab>('rel');

const TABS: { id: EcsTab; title: string; icon: typeof GitFork }[] = [
  { id: 'rel', title: 'Relationships', icon: GitFork },
  { id: 'sched', title: 'Schedule', icon: Workflow },
  { id: 'arch', title: 'Archetypes', icon: Layers },
];

export function EcsPage() {
  return (
    <div class="ecs-wrap">
      <div class="doc-tabs">
        {TABS.map((tab) => (
          <button
            class={`doc-tab${ecsTab.value === tab.id ? ' active' : ''}`}
            onClick={() => {
              ecsTab.value = tab.id;
            }}
            key={tab.id}
          >
            <Icon of={tab.icon} />
            {tab.title}
          </button>
        ))}
      </div>
      <div class={`ecs-body${ecsTab.value === 'rel' ? ' active' : ''}`}>
        {ecsTab.value === 'rel' && <RelationshipsTab />}
      </div>
      <div class={`ecs-body${ecsTab.value === 'sched' ? ' active' : ''}`}>
        {ecsTab.value === 'sched' && (
          <div class="pane" style="flex:1">
            Schedule
          </div>
        )}
      </div>
      <div class={`ecs-body${ecsTab.value === 'arch' ? ' active' : ''}`}>
        {ecsTab.value === 'arch' && (
          <div class="pane" style="flex:1">
            Archetypes
          </div>
        )}
      </div>
    </div>
  );
}
