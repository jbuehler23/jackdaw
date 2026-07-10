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

  it('refresh during an in-flight fetch chains a fresh fetch instead of joining the stale one', async () => {
    let calls = 0;
    const resolvers: Array<(v: number) => void> = [];
    const poll = createPoll(
      () => new Promise<number>((resolve) => {
        calls += 1;
        resolvers.push(resolve);
      }),
      1000,
    );
    poll.start();
    await Promise.resolve();
    expect(calls).toBe(1);

    const refreshed = poll.refresh();
    expect(calls).toBe(1); // chained refresh doesn't fetch until the in-flight one settles

    resolvers[0](1); // the stale, in-flight fetch resolves
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(2); // refresh() triggered a second, fresh fetcher invocation

    resolvers[1](42);
    await refreshed;
    expect(poll.data.value).toBe(42);
    poll.stop();
  });

  it('coalesces concurrent refresh calls into a single chained run', async () => {
    let calls = 0;
    const resolvers: Array<(v: number) => void> = [];
    const poll = createPoll(
      () => new Promise<number>((resolve) => {
        calls += 1;
        resolvers.push(resolve);
      }),
      1000,
    );
    poll.start();
    await Promise.resolve();
    expect(calls).toBe(1);

    const first = poll.refresh();
    const second = poll.refresh();

    resolvers[0](1);
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(2); // only one chained follow-up fetch, not two

    resolvers[1](7);
    await Promise.all([first, second]);
    expect(poll.data.value).toBe(7);
    poll.stop();
  });
});
