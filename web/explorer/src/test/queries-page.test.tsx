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

beforeEach(() => {
  page.value = 'queries';
  selectedEntity.value = null;
  queryMock.mockClear();
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
});
