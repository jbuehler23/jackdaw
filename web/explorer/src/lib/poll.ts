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

  return {
    data,
    error,
    refresh: () => run(),
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
