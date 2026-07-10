import { render, waitFor } from '@testing-library/preact';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { selectedEntity } from '../lib/state';
import type { ComponentSchema } from '../lib/registry';

const { TRANSFORM, transformSchema, POSITION, positionSchema, mutateComponents, insertComponents } = vi.hoisted(() => {
  const typePath = 'demo::Transform';
  const positionPath = 'avian3d::Position';
  const transformSchema: ComponentSchema = {
    typePath,
    shortName: 'Transform',
    fields: [{ name: 'translation', kind: 'vec3' as const }],
    defaultValue: () => ({ translation: { x: 0, y: 0, z: 0 } }),
  };
  // Single-field tuple struct (registry.ts unwraps a TupleStruct with one field to a
  // bare "value" row addressed by path === '') mimicking avian's Position(Vec3).
  const positionSchema: ComponentSchema = {
    typePath: positionPath,
    shortName: 'Position',
    fields: [{ name: '0', kind: 'vec3' as const }],
    defaultValue: () => ({ x: 0, y: 0, z: 0 }),
  };
  return {
    TRANSFORM: typePath,
    transformSchema,
    POSITION: positionPath,
    positionSchema,
    mutateComponents: vi.fn().mockResolvedValue(undefined),
    insertComponents: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: {
      ...mod.world,
      listComponents: vi.fn().mockResolvedValue([TRANSFORM]),
      getComponents: vi.fn().mockResolvedValue({ [TRANSFORM]: { translation: { x: 1, y: 2, z: 3 } } }),
      mutateComponents,
      insertComponents,
      removeComponents: vi.fn().mockResolvedValue(undefined),
    },
  };
});

vi.mock('../lib/registry', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/registry')>();
  return {
    ...mod,
    loadRegistry: vi.fn().mockResolvedValue(new Map([[TRANSFORM, transformSchema], [POSITION, positionSchema]])),
  };
});

import { Inspector, inspectorPoll } from '../components/Inspector';
import { world } from '../lib/brp';

beforeEach(() => {
  selectedEntity.value = null;
  inspectorPoll.stop();
  inspectorPoll.data.value = null;
  mutateComponents.mockClear();
  insertComponents.mockClear();
  vi.mocked(world.listComponents).mockResolvedValue([TRANSFORM]);
  vi.mocked(world.getComponents).mockResolvedValue({ [TRANSFORM]: { translation: { x: 1, y: 2, z: 3 } } });
});

describe('Inspector', () => {
  it('renders a vec3 field as three axis cells and writes typed edits via mutate_components', async () => {
    selectedEntity.value = 5;
    const { container, getByText } = render(<Inspector />);

    await waitFor(() => expect(getByText('translation')).toBeTruthy());

    const cells = container.querySelectorAll<HTMLElement>('.num-cell');
    expect(cells.length).toBe(3);
    const axisClasses = Array.from(cells).map((cell) => cell.querySelector('.axis')?.getAttribute('style'));
    expect(axisClasses.some((s) => s?.includes('--axis-x'))).toBe(true);
    expect(axisClasses.some((s) => s?.includes('--axis-y'))).toBe(true);
    expect(axisClasses.some((s) => s?.includes('--axis-z'))).toBe(true);

    const xInput = cells[0].querySelector('input') as HTMLInputElement;
    expect(xInput.value).toBe('1.0');

    // Native focus()/blur() rather than testing-library's fireEvent.blur: preact/compat
    // (loaded transitively via ContextMenu's createPortal) remaps onBlur to a bubbling
    // "focusout" listener, and @testing-library/dom's focusOut event construction doesn't
    // reach it (wrong event-name casing). Real focus/blur dispatches both natively.
    xInput.focus();
    xInput.value = '42';
    xInput.blur();

    await waitFor(() => expect(world.mutateComponents).toHaveBeenCalledWith(5, TRANSFORM, 'translation.x', 42));
  });

  it('writes whole-value edits for a single-field tuple-struct vec via insert_components, not mutate_components', async () => {
    vi.mocked(world.listComponents).mockResolvedValue([POSITION]);
    vi.mocked(world.getComponents).mockResolvedValue({ [POSITION]: { x: 1, y: 2, z: 3 } });

    selectedEntity.value = 7;
    const { container, getByText } = render(<Inspector />);

    await waitFor(() => expect(getByText('value')).toBeTruthy());

    const cells = container.querySelectorAll<HTMLElement>('.num-cell');
    expect(cells.length).toBe(3);
    const xInput = cells[0].querySelector('input') as HTMLInputElement;
    expect(xInput.value).toBe('1.0');

    xInput.focus();
    xInput.value = '42';
    xInput.blur();

    await waitFor(() =>
      expect(world.insertComponents).toHaveBeenCalledWith(7, { [POSITION]: { x: 42, y: 2, z: 3 } }),
    );
    expect(world.mutateComponents).not.toHaveBeenCalledWith(
      expect.anything(),
      expect.anything(),
      expect.stringMatching(/^\./),
      expect.anything(),
    );
  });
});
