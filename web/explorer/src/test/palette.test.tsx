import { fireEvent, render, waitFor } from '@testing-library/preact';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: {
      ...mod.world,
      query: vi.fn().mockResolvedValue([{ entity: 3, components: { 'bevy_ecs::name::Name': 'Grunt' }, has: {} }]),
    },
  };
});

import { CommandPalette, paletteOpen } from '../components/CommandPalette';
import { treePoll } from '../components/TreePanel';
import { page, selectedEntity } from '../lib/state';

beforeEach(async () => {
  page.value = 'stats';
  selectedEntity.value = null;
  paletteOpen.value = false;
  treePoll.stop();
  await treePoll.refresh();
});

describe('CommandPalette', () => {
  it('opens, filters to a tree entity by name, and selects it on Enter', async () => {
    paletteOpen.value = true;
    const { getByPlaceholderText } = render(<CommandPalette />);

    const input = getByPlaceholderText('Jump to entity, run a command');
    fireEvent.input(input, { target: { value: 'grunt' } });

    await waitFor(() => expect(document.querySelector('.pal-item')?.textContent).toContain('Grunt'));

    fireEvent.keyDown(input, { key: 'Enter' });

    expect(selectedEntity.value).toBe(3);
    expect(page.value).toBe('entities');
    expect(paletteOpen.value).toBe(false);
  });
});
