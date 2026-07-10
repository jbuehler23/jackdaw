import { fireEvent, render, waitFor } from '@testing-library/preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { spawnEntityMock, insertComponentsMock, queryMock } = vi.hoisted(() => ({
  spawnEntityMock: vi.fn(),
  insertComponentsMock: vi.fn(),
  queryMock: vi.fn().mockResolvedValue([]),
}));

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: {
      ...mod.world,
      query: queryMock,
      spawnEntity: spawnEntityMock,
      insertComponents: insertComponentsMock,
      mutateComponents: vi.fn(),
    },
  };
});

const { loadRegistryMock } = vi.hoisted(() => ({ loadRegistryMock: vi.fn() }));

vi.mock('../lib/registry', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/registry')>();
  return { ...mod, loadRegistry: loadRegistryMock };
});

import { ViewportPage, commitValueFor } from '../components/ViewportPage';
import type { SceneItem } from '../lib/viewport-scene';
import { viewportPoll } from '../lib/viewport-scene';
import { treePoll } from '../lib/treeData';
import { selectedEntity } from '../lib/state';
import { POINT_LIGHT, TRANSFORM } from '../lib/tree';

// jsdom has no canvas implementation: stub getContext with a recording object
// exposing every method/property the drawing code touches.
function stubCanvasContext() {
  const ctx = {
    setTransform: vi.fn(),
    clearRect: vi.fn(),
    beginPath: vi.fn(),
    closePath: vi.fn(),
    moveTo: vi.fn(),
    lineTo: vi.fn(),
    stroke: vi.fn(),
    fill: vi.fn(),
    arc: vi.fn(),
    strokeRect: vi.fn(),
    createRadialGradient: vi.fn(() => ({ addColorStop: vi.fn() })),
    strokeStyle: '',
    fillStyle: '',
    lineWidth: 1,
    globalAlpha: 1,
  };
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
  return ctx;
}

describe('commitValueFor', () => {
  it('commits the local translation plus drag delta, not the world position plus delta', () => {
    const item: SceneItem = {
      entity: 1,
      kind: 'box',
      pos: { x: 10, y: 0, z: 0 },
      localTranslation: { x: 2, y: 0, z: 0 },
      name: null,
    };
    expect(commitValueFor(item, 'x', 1)).toBe(3);
  });
});

describe('ViewportPage', () => {
  let rafSpy: ReturnType<typeof vi.fn>;
  let cafSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    stubCanvasContext();
    selectedEntity.value = null;
    queryMock.mockClear();
    queryMock.mockResolvedValue([]);
    spawnEntityMock.mockReset();
    insertComponentsMock.mockReset();
    loadRegistryMock.mockReset();
    viewportPoll.stop();
    viewportPoll.data.value = null;
    treePoll.stop();
    treePoll.data.value = null;
    vi.spyOn(treePoll, 'refresh').mockResolvedValue(undefined);
    vi.spyOn(viewportPoll, 'refresh').mockResolvedValue(undefined);

    rafSpy = vi.fn(() => 1);
    cafSpy = vi.fn();
    window.requestAnimationFrame = rafSpy as typeof window.requestAnimationFrame;
    window.cancelAnimationFrame = cafSpy as typeof window.cancelAnimationFrame;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('renders a canvas and an Add button', () => {
    const { container, getByText } = render(<ViewportPage />);
    expect(container.querySelector('canvas')).toBeTruthy();
    expect(getByText('Add')).toBeTruthy();
  });

  it('starts the RAF loop on mount and cancels it on unmount', () => {
    const { unmount } = render(<ViewportPage />);
    expect(rafSpy).toHaveBeenCalled();
    unmount();
    expect(cafSpy).toHaveBeenCalled();
  });

  it('spawns a point light with schema defaults and Transform at the camera target', async () => {
    spawnEntityMock.mockResolvedValue(42);
    loadRegistryMock.mockResolvedValue(
      new Map([[POINT_LIGHT, { typePath: POINT_LIGHT, shortName: 'PointLight', fields: 'marker', defaultValue: () => ({ intensity: 800 }) }]]),
    );

    const { getByText } = render(<ViewportPage />);
    fireEvent.click(getByText('Add'));
    fireEvent.click(getByText('Point light'));

    await waitFor(() => expect(spawnEntityMock).toHaveBeenCalledWith({}));
    await waitFor(() => expect(insertComponentsMock).toHaveBeenCalled());

    const [entity, components] = insertComponentsMock.mock.calls[0];
    expect(entity).toBe(42);
    expect(components[POINT_LIGHT]).toEqual({ intensity: 800 });
    expect(components[TRANSFORM]).toEqual({
      translation: { x: 0, y: 1, z: 0 },
      rotation: { x: 0, y: 0, z: 0, w: 1 },
      scale: { x: 1, y: 1, z: 1 },
    });

    await waitFor(() => expect(selectedEntity.value).toBe(42));
  });
});
