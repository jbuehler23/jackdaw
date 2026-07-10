import { render } from '@testing-library/preact';
import { describe, expect, it, vi } from 'vitest';

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
    world: {
      query: vi.fn().mockResolvedValue([]),
      listComponents: vi.fn().mockResolvedValue([]),
      getComponents: vi.fn().mockResolvedValue({}),
      mutateComponents: vi.fn().mockResolvedValue(undefined),
      insertComponents: vi.fn().mockResolvedValue(undefined),
      removeComponents: vi.fn().mockResolvedValue(undefined),
      spawnEntity: vi.fn().mockResolvedValue(0),
      despawnEntity: vi.fn().mockResolvedValue(undefined),
      reparentEntities: vi.fn().mockResolvedValue(undefined),
    },
    jackdaw: {
      ...mod.jackdaw,
      appInfo: vi.fn().mockResolvedValue({ app_name: 'Test Game', bevy_version: '0.19' }),
      diagnostics: vi.fn().mockResolvedValue({ fps: 60, frame_time_ms: 16.6, entity_count: 5 }),
      playback: vi.fn().mockResolvedValue({ paused: true }),
    },
    discoverCapabilities: vi.fn().mockResolvedValue(new Set(['jackdaw/diagnostics'])),
  };
});

import { App } from '../app';

describe('app shell', () => {
  it('renders', () => {
    const { container } = render(<App />);
    expect(container.textContent).toContain('jackdaw');
    expect(container.textContent).toContain('explorer');
  });
});
