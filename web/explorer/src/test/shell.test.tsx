import { fireEvent, render } from '@testing-library/preact';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { page, pollingPaused } from '../lib/state';

vi.mock('../lib/brp', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/brp')>();
  return {
    ...mod,
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
import { jackdaw } from '../lib/brp';

beforeEach(() => {
  page.value = 'entities';
  pollingPaused.value = false;
});

describe('shell', () => {
  it('switches pages from the rail', () => {
    const { getByTitle } = render(<App />);
    fireEvent.click(getByTitle('Queries'));
    expect(page.value).toBe('queries');
  });

  it('polling button toggles pollingPaused without calling playback', () => {
    const { getByTitle } = render(<App />);
    fireEvent.click(getByTitle(/polling/i));
    expect(pollingPaused.value).toBe(true);
    expect(jackdaw.playback).not.toHaveBeenCalled();
  });

  it('sim pause button calls jackdaw/playback', async () => {
    const { getByTitle } = render(<App />);
    fireEvent.click(getByTitle(/game simulation/i));
    expect(jackdaw.playback).toHaveBeenCalledWith('pause');
  });
});
