import { fireEvent, render, waitFor } from '@testing-library/preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { queryMock, entityBsnMock, schedulesMock, archetypesMock } = vi.hoisted(() => ({
  queryMock: vi.fn().mockResolvedValue([]),
  entityBsnMock: vi.fn().mockResolvedValue({ bsn: '' }),
  schedulesMock: vi.fn().mockResolvedValue({ schedules: [] }),
  archetypesMock: vi.fn().mockResolvedValue({ archetypes: [] }),
}));

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: { ...mod.world, query: queryMock },
    jackdaw: { ...mod.jackdaw, entityBsn: entityBsnMock, schedules: schedulesMock, archetypes: archetypesMock },
  };
});

const { loadRegistryMock } = vi.hoisted(() => ({ loadRegistryMock: vi.fn().mockResolvedValue(new Map()) }));
vi.mock('../lib/registry', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/registry')>();
  return { ...mod, loadRegistry: loadRegistryMock };
});

import { EcsPage } from '../components/EcsPage';
import {
  buildGraphData,
  canRunFrame,
  computeVisible,
  entityPath,
  pickNode,
  RelationshipsTab,
  shouldWarm,
} from '../components/RelationshipsTab';
import { ScheduleTab, afterNotes, orderSchedules } from '../components/ScheduleTab';
import { ArchetypesTab, seedFromArchetype } from '../components/ArchetypesTab';
import { fetchChips, withChips } from '../components/QueriesPage';
import { treePoll } from '../lib/treeData';
import { page, selectedEntity } from '../lib/state';
import { capabilities } from '../lib/connection';
import { NAME, CHILD_OF } from '../lib/tree';
import type { QueryRow } from '../lib/brp';
import type { ComponentSchema } from '../lib/registry';

// jsdom has no canvas implementation: stub getContext with a recording object
// exposing every method/property the drawing code touches.
function stubCanvasContext() {
  const ctx = {
    setTransform: vi.fn(),
    clearRect: vi.fn(),
    beginPath: vi.fn(),
    moveTo: vi.fn(),
    lineTo: vi.fn(),
    stroke: vi.fn(),
    fill: vi.fn(),
    arc: vi.fn(),
    roundRect: vi.fn(),
    measureText: vi.fn(() => ({ width: 10 })),
    fillText: vi.fn(),
    strokeStyle: '',
    fillStyle: '',
    lineWidth: 1,
    font: '',
  };
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
  return ctx;
}

const ROWS: QueryRow[] = [
  { entity: 1, components: { [NAME]: 'Root' }, has: {} },
  { entity: 2, components: { [NAME]: 'Child', [CHILD_OF]: 1 }, has: {} },
  { entity: 3, components: { [NAME]: 'Grandchild', [CHILD_OF]: 2 }, has: {} },
];

describe('buildGraphData', () => {
  it('maps every id to a parent (null for roots) and emits a ChildOf edge per non-root', () => {
    const graph = buildGraphData(ROWS);
    expect(graph.ids).toEqual([1, 2, 3]);
    expect(graph.parents.get(1)).toBeNull();
    expect(graph.parents.get(2)).toBe(1);
    expect(graph.parents.get(3)).toBe(2);
    expect(graph.edges).toEqual([
      { a: 2, b: 1, kind: 'child' },
      { a: 3, b: 2, kind: 'child' },
    ]);
    expect(graph.names.get(2)).toBe('Child');
  });

  it('treats a ChildOf pointing at a missing entity as a root', () => {
    const graph = buildGraphData([{ entity: 5, components: { [CHILD_OF]: 999 }, has: {} }]);
    expect(graph.parents.get(5)).toBeNull();
    expect(graph.edges).toEqual([]);
  });
});

describe('computeVisible', () => {
  it('keeps filter matches and their direct neighbors', () => {
    const graph = buildGraphData(ROWS);
    const visible = computeVisible(graph, 'root', false, null);
    expect(visible.has(1)).toBe(true);
    expect(visible.has(2)).toBe(true);
    expect(visible.has(3)).toBe(false);
  });

  it('restricts to the selection plus 2 hops when focus is on', () => {
    const graph = buildGraphData(ROWS);
    const visible = computeVisible(graph, '', true, 3);
    expect(visible).toEqual(new Set([1, 2, 3]));
  });
});

describe('entityPath', () => {
  it('walks the parent chain to build a path', () => {
    const graph = buildGraphData(ROWS);
    expect(entityPath(3, graph.parents, graph.names)).toBe('/Root/Child/Grandchild');
  });
});

describe('pickNode', () => {
  it('finds the nearest node within the pick radius, in graph coordinates', () => {
    const nodes = new Map([
      [1, { id: 1, x: 0, y: 0, vx: 0, vy: 0 }],
      [2, { id: 2, x: 100, y: 0, vx: 0, vy: 0 }],
    ]);
    const view = { x: 0, y: 0, k: 1 };
    // double-click / click selection both resolve through this same lookup,
    // so this stands in for the canvas dblclick handler jsdom can't drive.
    expect(pickNode({ x: 3, y: 2 }, view, nodes)?.id).toBe(1);
    expect(pickNode({ x: 500, y: 500 }, view, nodes)).toBeNull();
  });
});

describe('shouldWarm', () => {
  it('returns false when the graph is empty (no nodes)', () => {
    expect(shouldWarm(true, 0)).toBe(false);
  });

  it('returns true when needsWarm is true and the graph has nodes', () => {
    expect(shouldWarm(true, 3)).toBe(true);
  });

  it('returns false when needsWarm is false regardless of node count', () => {
    expect(shouldWarm(false, 100)).toBe(false);
  });
});

describe('canRunFrame', () => {
  // The RAF frame() body (graph seeding, warm start, fitView, stepping,
  // drawing) must never run against an unmeasured 0x0 canvas: seeding scatters
  // every node around (0, 0) and fitView clamps to its zoom floor there, and
  // since nothing re-warms once the flag is consumed the graph is stuck as a
  // squished blob. jsdom's getBoundingClientRect returns 0x0 by default, which
  // is exactly the bug condition, so these pure-function cases stand in for
  // it directly.
  it('is false for a 0x0 canvas', () => {
    expect(canRunFrame(0, 0)).toBe(false);
  });

  it('is false when only one axis has been measured', () => {
    expect(canRunFrame(800, 0)).toBe(false);
  });

  it('is true once both axes have a real measurement', () => {
    expect(canRunFrame(800, 600)).toBe(true);
  });
});

describe('EcsPage', () => {
  beforeEach(() => {
    stubCanvasContext();
    queryMock.mockClear();
    queryMock.mockResolvedValue([]);
    selectedEntity.value = null;
    page.value = 'ecs';
    capabilities.value = new Set();
    treePoll.stop();
    treePoll.data.value = null;
    vi.spyOn(treePoll, 'refresh').mockResolvedValue(undefined);
    window.requestAnimationFrame = vi.fn(() => 1) as unknown as typeof window.requestAnimationFrame;
    window.cancelAnimationFrame = vi.fn();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('renders the three doc-tabs and switches bodies on click', () => {
    const { getByText } = render(<EcsPage />);
    expect(getByText('Relationships')).toBeTruthy();
    expect(getByText('Schedule')).toBeTruthy();
    expect(getByText('Archetypes')).toBeTruthy();

    // Relationships is active by default: its canvas is mounted.
    expect(document.querySelector('canvas.rel-canvas')).toBeTruthy();

    fireEvent.click(getByText('Schedule'));
    expect(getByText('Schedule', { selector: 'div' })).toBeTruthy();
    expect(document.querySelector('canvas.rel-canvas')).toBeFalsy();
  });

  it('mounts the relationships canvas and shows the Fit button', () => {
    const { container, getByText } = render(<RelationshipsTab />);
    expect(container.querySelector('canvas.rel-canvas')).toBeTruthy();
    expect(getByText('Fit')).toBeTruthy();
  });

  it('renders the info card with the entity name and children count once selected', async () => {
    queryMock.mockResolvedValue(ROWS);
    await treePoll.refresh();
    selectedEntity.value = 1;

    const { getByText } = render(<RelationshipsTab />);

    await waitFor(() => expect(getByText('Root')).toBeTruthy());
    expect(document.querySelector('.rel-info.on')).toBeTruthy();
    expect(document.querySelector('.rel-info')?.textContent).toContain('children');
    expect(document.querySelector('.rel-info')?.textContent).toContain('1');
  });
});

describe('orderSchedules / afterNotes', () => {
  it('orders fixed lifecycle schedules first, then others alphabetically, skipping uninitialized extras', () => {
    const schedules = [
      { schedule: 'Zeta', initialized: true, systems: [], edges: [] },
      { schedule: 'Update', initialized: true, systems: [], edges: [] },
      { schedule: 'First', initialized: true, systems: [], edges: [] },
      { schedule: 'Alpha', initialized: false, systems: [], edges: [] },
    ];
    expect(orderSchedules(schedules).map((s) => s.schedule)).toEqual(['First', 'Update', 'Zeta']);
  });

  it('lists short names of systems with an edge into the given index, capped at 2', () => {
    const systems = [{ name: 'a::sys_one', sets: [] }, { name: 'b::sys_two', sets: [] }, { name: 'c::sys_three', sets: [] }];
    expect(afterNotes(systems, [[0, 2]], 2)).toEqual(['sys_one']);
    expect(afterNotes(systems, [[0, 1], [0, 2]], 2)).toEqual(['sys_one']);
  });

  it('caps at 2 names plus a "+N more" suffix', () => {
    const systems = Array.from({ length: 5 }, (_, i) => ({ name: `c::sys_${i}`, sets: [] }));
    const edges: [number, number][] = [[0, 4], [1, 4], [2, 4], [3, 4]];
    expect(afterNotes(systems, edges, 4)).toEqual(['sys_0', 'sys_1', '+2 more']);
  });
});

describe('ScheduleTab', () => {
  beforeEach(() => {
    schedulesMock.mockReset();
    schedulesMock.mockResolvedValue({ schedules: [] });
    capabilities.value = new Set(['jackdaw/schedules']);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    capabilities.value = new Set();
  });

  it('shows a capability hint when jackdaw/schedules is absent', () => {
    capabilities.value = new Set();
    const { getByText } = render(<ScheduleTab />);
    expect(getByText(/jackdaw\/schedules/)).toBeTruthy();
  });

  it('renders lanes from an enriched response, including set chips and an "after" note', async () => {
    schedulesMock.mockResolvedValue({
      schedules: [
        {
          schedule: 'Update',
          initialized: true,
          systems: [
            { name: 'brick_breaker::player_movement', sets: ['Physics'] },
            { name: 'brick_breaker::enemy_ai', sets: [] },
          ],
          edges: [[0, 1]],
        },
      ],
    });

    const { getByText } = render(<ScheduleTab />);

    await waitFor(() => expect(getByText('enemy_ai')).toBeTruthy());
    expect(getByText('player_movement')).toBeTruthy();
    expect(getByText('Physics')).toBeTruthy();
    expect(getByText(/after player_movement/)).toBeTruthy();
    expect(getByText(/timings not collected/i)).toBeTruthy();
  });

  it('renders a legacy string[] systems shape without edges or set chips', async () => {
    schedulesMock.mockResolvedValue({
      schedules: [{ schedule: 'Update', initialized: true, systems: [{ name: 'a::legacy_sys', sets: [] }], edges: [] }],
    });

    const { getByText, queryByText } = render(<ScheduleTab />);

    await waitFor(() => expect(getByText('legacy_sys')).toBeTruthy());
    expect(queryByText(/after /)).toBeFalsy();
  });
});

describe('seedFromArchetype', () => {
  it('splits components into up to 3 non-marker fetch and up to 2 marker with', () => {
    const registry = new Map<string, ComponentSchema>([
      ['a::Position', { typePath: 'a::Position', shortName: 'Position', fields: [], defaultValue: () => ({}) }],
      ['a::Velocity', { typePath: 'a::Velocity', shortName: 'Velocity', fields: [], defaultValue: () => ({}) }],
      ['a::Marker', { typePath: 'a::Marker', shortName: 'Marker', fields: 'marker', defaultValue: () => ({}) }],
    ]);
    const result = seedFromArchetype(['a::Position', 'a::Velocity', 'a::Marker'], registry);
    expect(result).toEqual({ fetch: ['a::Position', 'a::Velocity'], withList: ['a::Marker'] });
  });

  it('treats components missing from the registry as non-marker', () => {
    const result = seedFromArchetype(['unknown::Type'], null);
    expect(result).toEqual({ fetch: ['unknown::Type'], withList: [] });
  });
});

describe('ArchetypesTab', () => {
  beforeEach(() => {
    archetypesMock.mockClear();
    loadRegistryMock.mockReset();
    loadRegistryMock.mockResolvedValue(new Map());
    capabilities.value = new Set(['jackdaw/archetypes']);
    fetchChips.value = [];
    withChips.value = [];
    page.value = 'ecs';
  });

  afterEach(() => {
    vi.restoreAllMocks();
    capabilities.value = new Set();
  });

  it('shows a capability hint when jackdaw/archetypes is absent', () => {
    capabilities.value = new Set();
    const { getByText } = render(<ArchetypesTab />);
    expect(getByText(/jackdaw\/archetypes/)).toBeTruthy();
  });

  it('renders sorted rows with component chips and a meta summary', async () => {
    archetypesMock.mockResolvedValue({
      archetypes: [
        { components: ['a::Position', 'a::Marker'], entity_count: 5, bytes_per_entity: 12 },
        { components: ['a::Position'], entity_count: 2, bytes_per_entity: 8 },
      ],
    });

    const { getByText, getAllByText } = render(<ArchetypesTab />);

    await waitFor(() => expect(getByText('2 archetypes · 7 entities')).toBeTruthy());
    expect(getAllByText('Position').length).toBe(2);
    expect(getByText('Marker')).toBeTruthy();
  });

  it('seeds the query chips (non-markers to fetch, markers to with) and switches to the queries page', async () => {
    const registry = new Map<string, ComponentSchema>([
      ['a::Position', { typePath: 'a::Position', shortName: 'Position', fields: [], defaultValue: () => ({}) }],
      ['a::Marker', { typePath: 'a::Marker', shortName: 'Marker', fields: 'marker', defaultValue: () => ({}) }],
    ]);
    loadRegistryMock.mockResolvedValue(registry);
    archetypesMock.mockResolvedValue({
      archetypes: [{ components: ['a::Position', 'a::Marker'], entity_count: 3, bytes_per_entity: 12 }],
    });

    const { getByText } = render(<ArchetypesTab />);

    await waitFor(() => expect(getByText('Position')).toBeTruthy());
    await waitFor(() => expect(loadRegistryMock).toHaveBeenCalled());
    // Let the registry promise resolve into component state.
    await new Promise((resolve) => setTimeout(resolve, 0));

    fireEvent.click(getByText('query'));

    expect(page.value).toBe('queries');
    expect(fetchChips.value).toEqual(['a::Position']);
    expect(withChips.value).toEqual(['a::Marker']);
  });
});
