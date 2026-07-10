import { fireEvent, render, waitFor } from '@testing-library/preact';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { selectedEntity } from '../lib/state';

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: {
      ...mod.world,
      query: vi.fn().mockResolvedValue([
        { entity: 1, components: { 'bevy_ecs::name::Name': 'Root' }, has: {} },
        {
          entity: 2,
          components: { 'bevy_ecs::name::Name': 'Child', 'bevy_ecs::hierarchy::ChildOf': 1 },
          has: {},
        },
      ]),
    },
  };
});

import { TreePanel } from '../components/TreePanel';
import { treePoll } from '../lib/treeData';

beforeEach(() => {
  selectedEntity.value = null;
  treePoll.stop();
  treePoll.data.value = null;
});

describe('TreePanel', () => {
  it('renders rows from world.query and selects an entity on click', async () => {
    const { getByText } = render(<TreePanel />);

    await waitFor(() => expect(getByText('Root')).toBeTruthy());

    fireEvent.click(getByText('Root'));
    expect(selectedEntity.value).toBe(1);
  });
});
