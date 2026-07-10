// state.ts: cross-page UI state.
import { signal } from '@preact/signals';

export type Page = 'entities' | 'queries' | 'stats' | 'bsn' | 'viewport' | 'ecs';
export const page = signal<Page>('entities');
export const selectedEntity = signal<number | null>(null);
export const pollingPaused = signal(false);
export const simPaused = signal(false);
