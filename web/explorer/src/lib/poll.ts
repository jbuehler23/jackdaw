// poll.ts: interval polling into signals. Pausing is global (pollingPaused);
// refresh() bypasses the pause for user-initiated actions.
import { Signal, signal } from '@preact/signals';
import { pollingPaused } from './state';

export interface Poll<T> {
  data: Signal<T | null>;
  error: Signal<Error | null>;
  refresh(): Promise<void>;
  start(): void;
  stop(): void;
}

export function createPoll<T>(fetcher: () => Promise<T>, intervalMs: number): Poll<T> {
  const data = signal<T | null>(null);
  const error = signal<Error | null>(null);
  let timer: ReturnType<typeof setInterval> | null = null;
  let inFlight: Promise<void> | null = null;
  let pendingRefresh: Promise<void> | null = null;

  function run(): Promise<void> {
    if (inFlight) return inFlight;
    inFlight = (async () => {
      try {
        data.value = await fetcher();
        error.value = null;
      } catch (err) {
        error.value = err instanceof Error ? err : new Error(String(err));
      } finally {
        inFlight = null;
      }
    })();
    return inFlight;
  }

  // A refresh mid-flight must not resolve with the stale run it joined: chain a
  // fresh fetch after the in-flight one settles. Concurrent refresh() calls
  // coalesce onto the same chained follow-up rather than each queuing their own.
  function refresh(): Promise<void> {
    if (!inFlight) return run();
    if (!pendingRefresh) {
      pendingRefresh = inFlight.then(() => {
        pendingRefresh = null;
        return run();
      });
    }
    return pendingRefresh;
  }

  return {
    data,
    error,
    refresh,
    start() {
      if (timer !== null) return;
      void run();
      timer = setInterval(() => {
        if (!pollingPaused.value) void run();
      }, intervalMs);
    },
    stop() {
      if (timer !== null) clearInterval(timer);
      timer = null;
    },
  };
}
