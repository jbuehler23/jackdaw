import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { pollingPaused } from './state';
import { createPoll } from './poll';

beforeEach(() => {
  vi.useFakeTimers();
  pollingPaused.value = false;
});
afterEach(() => vi.useRealTimers());

describe('createPoll', () => {
  it('fetches immediately on start and then on the interval', async () => {
    let n = 0;
    const poll = createPoll(async () => ++n, 1000);
    poll.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(poll.data.value).toBe(1);
    await vi.advanceTimersByTimeAsync(2100);
    expect(poll.data.value).toBe(3);
    poll.stop();
  });

  it('pausing stops fetches; refresh works while paused', async () => {
    let n = 0;
    const poll = createPoll(async () => ++n, 1000);
    poll.start();
    await vi.advanceTimersByTimeAsync(0);
    pollingPaused.value = true;
    await vi.advanceTimersByTimeAsync(3000);
    expect(poll.data.value).toBe(1);
    await poll.refresh();
    expect(poll.data.value).toBe(2);
    poll.stop();
  });

  it('captures errors without clearing last data', async () => {
    let fail = false;
    const poll = createPoll(async () => {
      if (fail) throw new Error('down');
      return 7;
    }, 1000);
    poll.start();
    await vi.advanceTimersByTimeAsync(0);
    fail = true;
    await vi.advanceTimersByTimeAsync(1100);
    expect(poll.data.value).toBe(7);
    expect(poll.error.value?.message).toBe('down');
    poll.stop();
  });

  it('refresh during an in-flight fetch resolves with that fetch result', async () => {
    let resolveFetch: (v: number) => void = () => {};
    const poll = createPoll(
      () => new Promise<number>((resolve) => { resolveFetch = resolve; }),
      1000,
    );
    poll.start();
    const refreshed = poll.refresh();
    resolveFetch(42);
    await refreshed;
    expect(poll.data.value).toBe(42);
    poll.stop();
  });
});
