import { fireEvent, render, waitFor } from '@testing-library/preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { queryMock, entityBsnMock } = vi.hoisted(() => ({
  queryMock: vi.fn().mockResolvedValue([]),
  entityBsnMock: vi.fn().mockResolvedValue({ bsn: '' }),
}));

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: { ...mod.world, query: queryMock },
    jackdaw: { ...mod.jackdaw, entityBsn: entityBsnMock },
  };
});

import { EcsPage } from '../components/EcsPage';
import { buildGraphData, computeVisible, entityPath, pickNode, RelationshipsTab, shouldWarm } from '../components/RelationshipsTab';
import { treePoll } from '../lib/treeData';
import { page, selectedEntity } from '../lib/state';
import { NAME, CHILD_OF } from '../lib/tree';
import type { QueryRow } from '../lib/brp';

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

describe('EcsPage', () => {
  beforeEach(() => {
    stubCanvasContext();
    queryMock.mockClear();
    queryMock.mockResolvedValue([]);
    selectedEntity.value = null;
    page.value = 'ecs';
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
