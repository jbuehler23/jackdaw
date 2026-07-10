import { render, waitFor } from '@testing-library/preact';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { applyBsnMock, entityBsnMock } = vi.hoisted(() => ({
  applyBsnMock: vi.fn(),
  entityBsnMock: vi.fn(),
}));

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: { ...mod.world, query: vi.fn().mockResolvedValue([]) },
    jackdaw: { ...mod.jackdaw, applyBsn: applyBsnMock, entityBsn: entityBsnMock },
  };
});

import { BsnPage } from '../components/BsnPage';
import { BrpError } from '../lib/brp';
import { capabilities } from '../lib/connection';
import { selectedEntity } from '../lib/state';

beforeEach(() => {
  capabilities.value = new Set(['jackdaw/apply_bsn', 'jackdaw/entity_bsn']);
  selectedEntity.value = null;
  applyBsnMock.mockReset();
  entityBsnMock.mockReset();
});

describe('BsnPage', () => {
  it('applies the editor doc and renders an ok status', async () => {
    applyBsnMock.mockResolvedValue({ entities: [7] });
    const { getByText } = render(<BsnPage />);

    getByText('Apply to world').click();

    await waitFor(() => expect(applyBsnMock).toHaveBeenCalled());
    const [source] = applyBsnMock.mock.calls[0];
    expect(source).toContain('bevy_transform::components::transform::Transform');
    expect(source).toContain('bevy_light::point_light::PointLight');

    await waitFor(() => expect(document.querySelector('.bsn-status.ok')).toBeTruthy());
    expect(getByText(/Applied\. Spawned 1 entity: 7\./)).toBeTruthy();
  });

  it('shows a BrpError message in the status row', async () => {
    applyBsnMock.mockRejectedValue(new BrpError(-1, 'parse error at line 3'));
    const { getByText } = render(<BsnPage />);

    getByText('Apply to world').click();

    await waitFor(() => expect(document.querySelector('.bsn-status.err')).toBeTruthy());
    expect(getByText('parse error at line 3')).toBeTruthy();
  });
});
