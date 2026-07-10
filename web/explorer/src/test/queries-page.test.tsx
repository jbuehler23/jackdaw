import { fireEvent, render, waitFor } from '@testing-library/preact';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ComponentSchema } from '../lib/registry';

const { TRANSFORM, NAME, transformSchema, queryMock } = vi.hoisted(() => {
  const typePath = 'bevy_transform::components::transform::Transform';
  const namePath = 'bevy_ecs::name::Name';
  const transformSchema: ComponentSchema = {
    typePath,
    shortName: 'Transform',
    fields: [{ name: 'translation', kind: 'vec3' as const }],
    defaultValue: () => ({ translation: { x: 0, y: 0, z: 0 } }),
  };
  return {
    TRANSFORM: typePath,
    NAME: namePath,
    transformSchema,
    queryMock: vi.fn().mockResolvedValue([
      {
        entity: 1,
        components: { [namePath]: 'Root', [typePath]: { translation: { x: 1, y: 2, z: 3 } } },
        has: {},
      },
    ]),
  };
});

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: {
      ...mod.world,
      query: queryMock,
    },
  };
});

vi.mock('../lib/registry', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/registry')>();
  return {
    ...mod,
    loadRegistry: vi.fn().mockResolvedValue(new Map([[TRANSFORM, transformSchema]])),
  };
});

import { QueriesPage } from '../components/QueriesPage';
import { buildQueryParams } from '../lib/queries';
import { page, selectedEntity } from '../lib/state';
import { toasts } from '../lib/toasts';

beforeEach(() => {
  page.value = 'queries';
  selectedEntity.value = null;
  queryMock.mockClear();
  toasts.value = [];
});

describe('QueriesPage', () => {
  it('adds a fetch chip via autocomplete, runs the query, and renders rows', async () => {
    const { getByPlaceholderText, getByText } = render(<QueriesPage />);

    const fetchInput = getByPlaceholderText('component to fetch');
    await waitFor(() => {
      fireEvent.input(fetchInput, { target: { value: 'Transform' } });
      expect(document.querySelector('.autocomplete .ac-item')).toBeTruthy();
    });

    const acItem = document.querySelector('.autocomplete .ac-item') as HTMLButtonElement;
    fireEvent.mouseDown(acItem);

    fireEvent.click(getByText('Run query'));

    await waitFor(() =>
      expect(queryMock).toHaveBeenCalledWith(buildQueryParams([TRANSFORM, NAME], [], [])),
    );

    await waitFor(() => expect(getByText('Root')).toBeTruthy());
    expect(getByText('t: (1.0, 2.0, 3.0)')).toBeTruthy();
  });

  it('routes auto-refresh poll failures into the inline note instead of a toast', async () => {
    const { getByText, getByPlaceholderText, container } = render(<QueriesPage />);

    // fetchChips/queryRows live at module scope in QueriesPage.tsx and aren't reset
    // between tests, so the previous test's Transform chip and results table are
    // still present here. Clear the chip first (empty inputs make Run query reset
    // queryRows to null) so the note branch below is actually the one rendered.
    const removeChip = container.querySelector('.chip button') as HTMLButtonElement | null;
    if (removeChip) fireEvent.click(removeChip);
    fireEvent.click(getByText('Run query'));
    await waitFor(() => expect(getByText('Add at least one component to fetch or filter on.')).toBeTruthy());

    const fetchInput = getByPlaceholderText('component to fetch');
    await waitFor(() => {
      fireEvent.input(fetchInput, { target: { value: 'Transform' } });
      expect(document.querySelector('.autocomplete .ac-item')).toBeTruthy();
    });
    const acItem = document.querySelector('.autocomplete .ac-item') as HTMLButtonElement;
    fireEvent.mouseDown(acItem);

    queryMock.mockRejectedValueOnce(new Error('connection lost'));
    // Enabling auto-refresh fires the poll immediately (before any interval tick).
    fireEvent.click(getByText('Auto-refresh'));

    await waitFor(() => expect(queryMock).toHaveBeenCalled());
    await waitFor(() => expect(getByText('connection lost')).toBeTruthy());
    expect(toasts.value).toEqual([]);
  });
});
