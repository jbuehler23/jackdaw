// ScheduleTab.tsx: run-order lanes for each app schedule. Fed by
// `jackdaw/schedules`; the server never reports per-system timings, so this
// shows structure (order, sets, "after" deps) only.
import { useEffect } from 'preact/hooks';
import { Workflow, Zap } from 'lucide-preact';
import { Icon } from './Icon';
import { jackdaw, type ScheduleInfo, type ScheduleSystem } from '../lib/brp';
import { createPoll } from '../lib/poll';
import { capabilities } from '../lib/connection';

const FIXED_ORDER = ['First', 'PreUpdate', 'Update', 'FixedUpdate', 'PostUpdate', 'Last'];

const schedulePoll = createPoll(jackdaw.schedules, 2000);

/** Orders schedules: the fixed lifecycle order first (kept even if
 * uninitialized, so the lanes stay stable across frames), then any other
 * schedule alphabetically, skipping ones that aren't initialized. */
export function orderSchedules(schedules: ScheduleInfo[]): ScheduleInfo[] {
  const byName = new Map(schedules.map((s) => [s.schedule, s]));
  const fixed: ScheduleInfo[] = [];
  for (const name of FIXED_ORDER) {
    const s = byName.get(name);
    if (s) fixed.push(s);
  }
  const rest = schedules
    .filter((s) => !FIXED_ORDER.includes(s.schedule) && s.initialized)
    .sort((a, b) => a.schedule.localeCompare(b.schedule));
  return [...fixed, ...rest];
}

/** Short names (last `::` segment) of the systems with an edge into `index`,
 * capped at 2 with a "+N more" suffix once there are more. */
export function afterNotes(systems: ScheduleSystem[], edges: [number, number][], index: number): string[] {
  const beforeIndices = edges.filter(([, after]) => after === index).map(([before]) => before);
  const names = beforeIndices.map((i) => shortSystemName(systems[i]?.name ?? ''));
  if (names.length <= 2) return names;
  return [...names.slice(0, 2), `+${names.length - 2} more`];
}

function shortSystemName(name: string): string {
  return name.split('::').pop() ?? name;
}

function crateName(name: string): string {
  return name.split('::')[0] ?? name;
}

export function ScheduleTab() {
  const canSchedules = capabilities.value.has('jackdaw/schedules');

  useEffect(() => {
    if (!canSchedules) return;
    schedulePoll.start();
    return () => schedulePoll.stop();
  }, [canSchedules]);

  if (!canSchedules) {
    return (
      <div class="pane" style="flex:1">
        <div class="pane-header">
          <Icon of={Workflow} />
          Schedule
        </div>
        <div class="stats-wrap">
          <div class="stats-note">
            <Icon of={Zap} />
            <span>
              Served by the <code>jackdaw/schedules</code> method from <code>JackdawRemotePlugin</code>. Apps running
              plain BRP still get the tree, inspector, and queries; this page shows an upgrade hint instead.
            </span>
          </div>
        </div>
      </div>
    );
  }

  const schedules = orderSchedules(schedulePoll.data.value?.schedules ?? []);

  return (
    <div class="sched-scroll">
      <div class="sched-lanes">
        {schedules.map((sched) => (
          <div class="sched-set" key={sched.schedule}>
            <h4>{sched.schedule}</h4>
            {!sched.initialized ? (
              <div class="sys-box">not initialized</div>
            ) : (
              sched.systems.map((sys, index) => {
                const after = afterNotes(sched.systems, sched.edges, index);
                return (
                  <div class="sys-box" title={sys.name} key={`${sys.name}-${index}`}>
                    <div class="sys-name">{shortSystemName(sys.name)}</div>
                    <div class="sys-crate">{crateName(sys.name)}</div>
                    {sys.sets.length > 0 && (
                      <div>
                        {sys.sets.map((set) => (
                          <span class="comp-chip" key={set}>
                            {set}
                          </span>
                        ))}
                      </div>
                    )}
                    {after.length > 0 && <div class="sys-crate">after {after.join(', ')}</div>}
                  </div>
                );
              })
            )}
          </div>
        ))}
      </div>
      <div class="sched-note">Timings not collected by this app.</div>
    </div>
  );
}
